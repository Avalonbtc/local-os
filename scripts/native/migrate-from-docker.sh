#!/usr/bin/env bash
# Move an existing Docker Compose install of RigDeck to the native (source + systemd) setup.
#   cd <repo with compose.yml, .env and secrets/master.key>
#   sudo bash scripts/native/migrate-from-docker.sh        (run it in a terminal: it may ask)
# Keeps: database, SSH/BMC credentials (same master key), packages/BIOS data, Caddy's HTTPS CA,
# the access mode (HTTPS / SSH tunnel / cloudflared) and listen address. Docker volumes are left
# untouched, so rolling back is `sudo systemctl disable --now rigdeck caddy && docker compose up -d`.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m错误：%s\033[0m\n' "$*" >&2; exit 1; }
as_postgres() { (cd / && runuser -u postgres -- "$@"); }
[[ $EUID -eq 0 ]] || die "请用 sudo 运行"
[[ -f compose.yml && -f .env && -f secrets/master.key ]] || die "在原 Docker 部署的目录里运行（需要 compose.yml、.env、secrets/master.key）"
command -v docker >/dev/null || die "找不到 docker"

# .env may come from a Windows checkout (CRLF) or carry quotes.
env_value() { sed -n "s/^$1=//p" .env | tail -1 | tr -d '\r' | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'$/\1/"; }
compose_files="$(env_value COMPOSE_FILE)"
host="$(env_value RIGDECK_HOST)"; host="${host:-rigdeck.local}"
listen="$(env_value RIGDECK_LISTEN)"; listen="${listen:-0.0.0.0}"
mode=https
[[ "$compose_files" == *ssh-tunnel* ]] && mode=tunnel
[[ "$compose_files" == *cloudflared* ]] && mode=cloudflared
echo "原部署：mode=$mode host=$host listen=$listen"

# Refuse anything that would make stored credentials undecryptable or change the access mode.
if [[ -f /etc/rigdeck/master.key ]] && ! cmp -s <(tr -d '\r\n' < secrets/master.key) <(tr -d '\r\n' < /etc/rigdeck/master.key); then
  die "/etc/rigdeck/master.key 与 secrets/master.key 不同。先确认哪把是正确的；用错密钥会导致所有 SSH/BMC 凭据无法解密"
fi
if [[ -f /etc/rigdeck/rigdeck.env ]] && ! grep -q "^# RigDeck service configuration (scripts/native/install.sh, mode=$mode)" /etc/rigdeck/rigdeck.env; then
  die "/etc/rigdeck/rigdeck.env 已存在，但不是 mode=$mode。确认后删除它再运行"
fi

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
work="$repo/backups/docker-to-native-$stamp"
install -d -m 700 "$work"

say "1/6 安装原生环境并编译（Docker 部署在此期间照常运行）"
bash "$repo/scripts/native/install.sh" --mode "$mode" --host "$host" --listen "$listen" --no-start
systemctl stop rigdeck 2>/dev/null || true

# Ask before anything is stopped, and never without a terminal to answer.
tables="$(as_postgres psql -d rigdeck -tAc "SELECT count(*) FROM pg_tables WHERE schemaname='public'")"
if [[ "$tables" != 0 ]]; then
  [[ -t 0 ]] || die "本机 rigdeck 数据库已有 $tables 张表；请在终端里交互运行本脚本"
  read -rp "本机 rigdeck 数据库已有 $tables 张表，清空后用容器数据覆盖？输入 yes 继续：" answer
  [[ "$answer" == yes ]] || die "已取消，Docker 部署没有改动"
fi

say "2/6 停止面板容器（数据库容器最后停止，卷保留）"
docker compose stop app
[[ "$mode" == https ]] && docker compose stop https

say "3/6 导出数据库和数据（此时不会再有写入）"
docker compose exec -T postgres pg_dump -U rigdeck -d rigdeck -Fc > "$work/database.dump"
[[ -s "$work/database.dump" ]] || die "数据库导出为空；执行 docker compose start 可恢复原部署"
app_container="$(docker compose ps -aq app)"
docker cp "$app_container:/data/." "$work/data/"
if [[ "$mode" == https ]]; then
  https_container="$(docker compose ps -aq https)"
  if [[ -n "$https_container" ]]; then
    docker cp "$https_container:/data/caddy/." "$work/caddy/" || echo "（没有 Caddy 数据，将生成新的 HTTPS 证书）"
  fi
fi
docker compose stop

say "4/6 恢复数据库到本机 PostgreSQL 18"
if [[ "$tables" != 0 ]]; then
  as_postgres dropdb rigdeck
  as_postgres createdb -O rigdeck rigdeck
fi
# One transaction: a failed restore leaves an empty database, never a half-restored one.
as_postgres pg_restore --exit-on-error --single-transaction --no-owner --role=rigdeck -d rigdeck < "$work/database.dump"

say "5/6 迁移数据目录和证书"
run_user="$(stat -c %U /var/lib/rigdeck-controller)"
cp -a "$work/data/." /var/lib/rigdeck-controller/
chown -R "$run_user":"$(id -gn "$run_user")" /var/lib/rigdeck-controller
chmod 750 /var/lib/rigdeck-controller
if [[ "$mode" == https && -d "$work/caddy" ]]; then
  install -d /var/lib/caddy/.local/share/caddy
  cp -a "$work/caddy/." /var/lib/caddy/.local/share/caddy/
  chown -R caddy:caddy /var/lib/caddy/.local/share/caddy
fi

say "6/6 启动原生服务"
systemctl restart rigdeck
if [[ "$mode" == https ]]; then
  systemctl restart caddy
fi
sleep 5
systemctl --no-pager --lines=10 status rigdeck || true

rollback_services=rigdeck
[[ "$mode" == https ]] && rollback_services="rigdeck caddy"
cat <<DONE

迁移完成。原容器已停止但没有删除，确认一切正常后可以执行：
  docker compose down          # 删除容器（卷仍保留）
回滚方法：
  sudo systemctl disable --now $rollback_services && docker compose up -d
迁移时的备份在：$work（包含数据库，请妥善保管）
DONE
