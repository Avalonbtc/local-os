#!/usr/bin/env bash
# Install RigDeck on an Ubuntu/Debian controller WITHOUT Docker: PostgreSQL 18, Node 22, Rust,
# Caddy (HTTPS mode) and a systemd service that runs straight from this source tree.
#
#   sudo bash scripts/native/install.sh --host rigdeck.local            # HTTPS via Caddy (default)
#   sudo bash scripts/native/install.sh --host rigdeck.local --listen 192.168.1.5   # Caddy on one address
#   sudo bash scripts/native/install.sh --mode tunnel                   # SSH tunnel only (127.0.0.1:18082)
#   sudo bash scripts/native/install.sh --mode cloudflared --host panel.example.com
#
# Re-running is safe: existing config, keys and database are kept.
# --no-start prepares everything but does not start the service (used by migrate-from-docker.sh).
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
mode="https"
host="rigdeck.local"
listen="0.0.0.0"
start=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host) host="$2"; shift 2 ;;
    --mode) mode="$2"; shift 2 ;;
    --listen) listen="$2"; shift 2 ;;
    --no-start) start=0; shift ;;
    -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "未知参数：$1" >&2; exit 2 ;;
  esac
done
# runuser works as plain root too (no sudo needed); HOME must point at the user's for cargo.
as_user() { runuser -u "$run_user" -- env HOME="$user_home" "$@"; }
as_postgres() { (cd / && runuser -u postgres -- "$@"); }
say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31m错误：%s\033[0m\n' "$*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || die "请用 sudo 运行"
[[ "$mode" =~ ^(https|tunnel|cloudflared)$ ]] || die "--mode 只能是 https、tunnel 或 cloudflared"
[[ "$host" =~ ^[A-Za-z0-9.-]+$ ]] || die "--host 只能是主机名或 IPv4 地址"
[[ "$listen" =~ ^[0-9.]+$ ]] || die "--listen 只能是 IPv4 地址"
# The service runs as (and builds as) the owner of the source tree, like restart.sh assumes.
run_user="$(stat -c %U "$repo")"
[[ "$run_user" != root ]] || die "源码目录属于 root。请先交给普通用户：sudo chown -R <用户>: $repo"
if find "$repo/target" "$repo/frontend/node_modules" "$repo/frontend/dist" -maxdepth 0 ! -user "$run_user" 2>/dev/null | grep -q .; then
  die "target/、node_modules 或 dist 不属于 $run_user，请先执行：sudo chown -R $run_user: $repo"
fi
run_group="$(id -gn "$run_user")"
user_home="$(getent passwd "$run_user" | cut -d: -f6)"
# shellcheck disable=SC1091
. /etc/os-release
codename="${VERSION_CODENAME:-}"
[[ -n "$codename" ]] || die "无法识别系统版本（需要 Ubuntu 22.04/24.04 或 Debian 12）"
export DEBIAN_FRONTEND=noninteractive

say "安装系统依赖"
apt-get update
apt-get install -y ca-certificates curl gnupg build-essential pkg-config git python3 ipmitool jq sudo

if [[ ! -x /usr/lib/postgresql/18/bin/postgres ]]; then
  say "安装 PostgreSQL 18（与原容器同版本，旧备份可直接恢复）"
  install -d /usr/share/postgresql-common/pgdg
  curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc -o /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc
  echo "deb [signed-by=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc] https://apt.postgresql.org/pub/repos/apt ${codename}-pgdg main" \
    > /etc/apt/sources.list.d/pgdg.list
  apt-get update
  apt-get install -y postgresql-18 postgresql-client-18
fi
systemctl enable --now postgresql

if ! command -v node >/dev/null || [[ "$(node -p 'process.versions.node.split(".")[0]')" -lt 22 ]]; then
  say "安装 Node.js 22"
  install -d /etc/apt/keyrings
  curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor --yes -o /etc/apt/keyrings/nodesource.gpg
  echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_22.x nodistro main" \
    > /etc/apt/sources.list.d/nodesource.list
  apt-get update
  apt-get install -y nodejs
fi

if [[ ! -x "$user_home/.cargo/bin/cargo" ]]; then
  say "为 $run_user 安装 Rust 1.91.1（rustup）"
  as_user bash -c 'curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.91.1'
fi
as_user "$user_home/.cargo/bin/rustup" toolchain install 1.91.1 --profile minimal
as_user bash -c 'cd "$1" && "$HOME/.cargo/bin/rustup" override set 1.91.1' bash "$repo"

if [[ "$mode" == https ]] && ! command -v caddy >/dev/null; then
  say "安装 Caddy"
  curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/gpg.key | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -fsSL https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt > /etc/apt/sources.list.d/caddy-stable.list
  apt-get update
  apt-get install -y caddy
fi

say "配置 /etc/rigdeck"
install -d -m 750 -o root -g "$run_group" /etc/rigdeck
if [[ ! -f /etc/rigdeck/master.key ]]; then
  if [[ -f "$repo/secrets/master.key" ]]; then
    # Existing Docker install: the same key must be used, or stored credentials can't be decrypted.
    install -m 640 -o root -g "$run_group" "$repo/secrets/master.key" /etc/rigdeck/master.key
    echo "已沿用 secrets/master.key"
  else
    (umask 077; head -c 32 /dev/urandom | base64 > /etc/rigdeck/master.key)
    chown root:"$run_group" /etc/rigdeck/master.key
    chmod 640 /etc/rigdeck/master.key
    echo "已生成新的主密钥 /etc/rigdeck/master.key —— 请另外备份"
  fi
fi

if [[ ! -f /etc/rigdeck/rigdeck.env ]]; then
  db_password="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
  case "$mode" in
    https)
      bind="127.0.0.1:8080"; origin="https://$host"; extra="" ;;
    tunnel)
      bind="127.0.0.1:18082"; origin="http://localhost:18081"; extra="RIGDECK_INSECURE_LOCALHOST=1" ;;
    cloudflared)
      bind="127.0.0.1:18082"; origin="https://$host"; extra="RIGDECK_TRUST_PROXY_HEADERS=1" ;;
  esac
  cat > /etc/rigdeck/rigdeck.env <<ENV
# RigDeck service configuration (scripts/native/install.sh, mode=$mode)
DATABASE_URL=postgres://rigdeck:${db_password}@127.0.0.1:5432/rigdeck
RIGDECK_BIND=$bind
RIGDECK_ORIGIN=$origin
RIGDECK_MASTER_KEY_FILE=/etc/rigdeck/master.key
RIGDECK_DATA_DIR=/var/lib/rigdeck-controller
RIGDECK_STATIC_DIR=$repo/frontend/dist
RIGDECK_SUM_PATH=$repo/tools/sum/sum
RUST_LOG=info,sqlx=warn
$extra
ENV
  chown root:"$run_group" /etc/rigdeck/rigdeck.env
  chmod 640 /etc/rigdeck/rigdeck.env
  echo "已写入 /etc/rigdeck/rigdeck.env（mode=$mode）"
fi
db_password="$(sed -n 's|^DATABASE_URL=postgres://rigdeck:\([^@]*\)@.*|\1|p' /etc/rigdeck/rigdeck.env)"
[[ -n "$db_password" ]] || die "/etc/rigdeck/rigdeck.env 里的 DATABASE_URL 格式不对"

say "准备数据库 rigdeck"
if ! as_postgres psql -tAc "SELECT 1 FROM pg_roles WHERE rolname='rigdeck'" | grep -q 1; then
  as_postgres psql -v ON_ERROR_STOP=1 -c "CREATE ROLE rigdeck LOGIN PASSWORD '$db_password'"
else
  # Keep the role in step with rigdeck.env (the password is hex, safe to inline).
  as_postgres psql -v ON_ERROR_STOP=1 -qc "ALTER ROLE rigdeck LOGIN PASSWORD '$db_password'"
fi
if ! as_postgres psql -tAc "SELECT 1 FROM pg_database WHERE datname='rigdeck'" | grep -q 1; then
  as_postgres createdb -O rigdeck rigdeck
fi

install -d -m 750 -o "$run_user" -g "$run_group" /var/lib/rigdeck-controller

say "安装 systemd 服务（运行用户 $run_user，源码 $repo）"
tr -d '\r' < "$repo/deploy/native/rigdeck.service.in" \
  | sed -e "s|@USER@|$run_user|g" -e "s|@GROUP@|$run_group|g" -e "s|@REPO@|$repo|g" \
  > /etc/systemd/system/rigdeck.service
systemctl daemon-reload
systemctl enable rigdeck >/dev/null

if [[ "$mode" == https ]]; then
  tr -d '\r' < "$repo/deploy/native/Caddyfile.in" | sed -e "s|@HOST@|$host|g" -e "s|@LISTEN@|$listen|g" > /etc/caddy/Caddyfile
  systemctl enable caddy >/dev/null
fi

say "首次编译（后端 release + 前端），需要几分钟"
as_user bash "$repo/scripts/native/build.sh"

if [[ $start -eq 1 ]]; then
  say "启动服务"
  systemctl restart rigdeck
  [[ "$mode" == https ]] && systemctl restart caddy
  sleep 3
  systemctl --no-pager --lines=5 status rigdeck || true
fi

cat <<DONE

完成。常用命令：
  创建管理员：  cd $repo && read -rsp '密码: ' P && RIGDECK_ADMIN_PASSWORD="\$P" bash scripts/native/rigdeck.sh create-admin admin
  改完代码后：  bash scripts/native/restart.sh     （先编译，成功后才重启）
  查看日志：    journalctl -u rigdeck -f
  配置文件：    /etc/rigdeck/rigdeck.env   主密钥：/etc/rigdeck/master.key（请另外备份）
DONE
if [[ "$mode" == https ]]; then
  echo "  HTTPS 根证书：/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt（导入到访问设备）"
fi
