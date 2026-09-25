"""Controller-only SUM adapter. Never enables reboot or accepts arbitrary commands."""
from collections import Counter
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET


def atomic(path, value):
    temporary = path.with_suffix('.tmp')
    with open(temporary, 'w', encoding='utf-8') as f:
        json.dump(value, f, ensure_ascii=False)
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)


def parse_config(path):
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError('BIOS 配置超过 16 MiB')
    root = ET.parse(path).getroot()
    fields = []
    menu_counts = Counter()
    def walk(element, menus):
        if element.tag == 'Menu':
            menus = menus + [element.attrib['name']]
            menu_counts[tuple(menus)] += 1
        if element.tag == 'Setting':
            name = element.get('name', '')
            # Passwords, raw strings and ordered boot lists are never exposed or edited.
            kind = element.get('type')
            if kind not in ('Option', 'Numeric') or re.search('password|secret|key enrollment', name, re.I):
                return
            value = element.get('selectedOption' if kind == 'Option' else 'numericValue')
            if value is None:
                return
            identity = json.dumps(menus + [name], ensure_ascii=False)
            info = element.find('Information')
            def text(tag):
                return info.findtext(tag, '') if info is not None else ''
            def number(tag):
                try: return int(text(tag), 0)
                except ValueError: return None
            fields.append(dict(id=identity, name=name, group=' / '.join(menus), menus=menus,
                value=value, kind=kind, options=[o.text for o in element.findall('./Information/AvailableOptions/Option') if o.text],
                minimum=number('MinValue'), maximum=number('MaxValue'), step=number('StepSize'),
                help=text('Help'), condition=text('WorkIf'),
                license=text('LicenseRequirement')))
        for child in element:
            walk(child, menus)
    walk(root, [])
    counts = Counter(f['id'] for f in fields)
    # Vendor XML repeats serial-port menus and boot entries; never guess which it means.
    fields = [f for f in fields if counts[f['id']] == 1 and all(
        menu_counts[tuple(f['menus'][:i])] == 1 for i in range(1, len(f['menus']) + 1))]
    if not fields:
        raise ValueError('未找到可编辑的 BIOS 设置')
    return fields


def revision(fields, pending):
    return hashlib.sha256(json.dumps([fields, pending], sort_keys=True).encode()).hexdigest()


def changes_xml(fields, changes):
    known = {f['id']: f for f in fields}
    if not changes or len(changes) > 64:
        raise ValueError('每次需修改 1–64 个设置')
    root = ET.Element('BiosCfg')
    for key, value in changes.items():
        f = known.get(key)
        if not f or not isinstance(value, str):
            raise ValueError('未知 BIOS 设置或值类型错误')
        if f['kind'] == 'Option':
            if value not in f['options']: raise ValueError(f['name'] + ': 选项无效')
        else:
            if not re.fullmatch(r'[0-9]+', value): raise ValueError(f['name'] + ': 需要非负整数')
            number = int(value)
            if (f['minimum'] is not None and number < f['minimum']) or (f['maximum'] is not None and number > f['maximum']):
                raise ValueError(f['name'] + ': 超出 BIOS 允许范围')
            if f['step'] and (number - (f['minimum'] or 0)) % f['step']:
                raise ValueError(f['name'] + ': 不符合步长')
        parent = root
        for menu in f['menus']:
            child = next((c for c in parent if c.tag == 'Menu' and c.get('name') == menu), None)
            if child is None: child = ET.SubElement(parent, 'Menu', name=menu)
            parent = child
        attr = 'selectedOption' if f['kind'] == 'Option' else 'numericValue'
        ET.SubElement(parent, 'Setting', {'name': f['name'], 'type': f['kind'], attr: value})
    return ET.ElementTree(root)


def main(request):
    import fcntl
    os.umask(0o077)
    root = Path(request['root']) / str(uuid.UUID(request['machine_id']))
    root.mkdir(parents=True, exist_ok=True)
    operation = str(uuid.UUID(request['operation_id']))
    op = root / operation
    lock = open(root / 'lock', 'a')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lock.close()
        if request.get('reconcile'):
            # Possibly this very operation, still running in an orphaned adapter: never guess.
            return dict(status='unknown', output='', truncated=False, error='该机器的 SUM 操作仍在进行，请稍后对账')
        # A fresh operation has not dispatched anything; failing it cannot hide a write.
        return dict(status='failed', output='', truncated=False, error='该机器有另一个 SUM 操作正在进行，本次未执行，请稍后重试')
    try:
        journal = op / 'result.json'
        if journal.exists():
            old = json.loads(journal.read_text(encoding='utf-8'))
            if old['status'] == 'running':
                old.update(status='unknown', error='上次 SUM 操作中断，禁止自动重复写入；请读取 BIOS 后核实')
            return old
        if request.get('reconcile'):
            return dict(status='unknown', output='', truncated=False, error='无本地操作记录，禁止自动重发 BIOS 写入')
        # A crash between mkdir and the first journal write left an empty directory: no SUM call ran.
        op.mkdir(exist_ok=True)
        result = dict(status='running', output='', truncated=False, result=None, error=None)
        atomic(journal, result)
        writing = False
        password = request['password']
        snapshot = root / 'snapshot.json'
        prior = json.loads(snapshot.read_text(encoding='utf-8')) if snapshot.exists() else {}
        try:
            binary = request['binary']
            if not Path(binary).is_file(): raise ValueError('主控未安装 Supermicro SUM，请配置 RIGDECK_SUM_PATH')
            host = request['host'].removeprefix('https://').removeprefix('http://').rstrip('/')
            if not re.fullmatch(r'[a-zA-Z0-9_.:-]+', host) or host.startswith('-'):
                raise ValueError('SUM 需要有效的 BMC 主机地址')
            with tempfile.TemporaryDirectory(prefix='rigdeck-sum-') as temp:
                passfile = Path(temp) / 'password'
                passfile.write_text(password + '\n')
                def run(command, filename=None):
                    args = [binary, '-i', host, '-u', request['username'], '-f', str(passfile), '-c', command]
                    if filename: args += ['--file', str(filename)]
                    try:
                        proc = subprocess.run(args, cwd=op, stdin=subprocess.DEVNULL, capture_output=True, timeout=120)
                    except subprocess.TimeoutExpired:
                        raise RuntimeError('SUM 超时，写入结果可能不明；请重新读取核实')
                    output = (proc.stdout + proc.stderr).decode(errors='replace').replace(password, '[REDACTED]')
                    output = re.sub(r'\x1b\[[0-9;]*m', '', output)
                    (op / (command + '.log')).write_text(output, encoding='utf-8')
                    if proc.returncode:
                        raise RuntimeError('SUM 返回错误 ' + str(proc.returncode) + ': ' + output[-1800:])
                    return output
                license_text = run('QueryProductKey')
                licenses = [key for key in ['SFT-OOB-LIC', 'SFT-DCMS-SINGLE'] if key in license_text]
                if not licenses: raise ValueError('该 BMC 未激活 OOB/DCMS')
                run('GetCurrentBiosCfg', op / 'before.xml')
                fields = parse_config(op / 'before.xml')
                pending = prior.get('pending', {})
                values = {f['id']: f['value'] for f in fields}
                if pending and all(values.get(k) == v for k, v in pending.items()): pending = {}
                if request['action']['kind'] == 'bios_read' and request['action'].get('discard_pending'):
                    # Operator confirmed the machine rebooted and the firmware kept its values (for
                    # example a conditional setting it ignored). Otherwise pending blocks writes forever.
                    pending = {}
                current_revision = revision(fields, pending)
                view = dict(machine_id=request['machine_id'], settings=fields, licenses=licenses,
                    revision=current_revision, pending=pending, observed_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    last_operation=operation)
                if request['action']['kind'] == 'bios_write':
                    action = request['action']
                    if action['revision'] != current_revision: raise ValueError('BIOS 配置已变化，请重新读取后再提交')
                    if pending: raise ValueError('已有待生效修改，请重启并回读验证后再修改')
                    changes = action['changes']
                    changes_xml(fields, changes).write(op / 'changes.xml', encoding='utf-8', xml_declaration=True)
                    changes_xml(fields, {k: values[k] for k in changes}).write(op / 'rollback.xml', encoding='utf-8', xml_declaration=True)
                    # Persist intent BEFORE SUM. Even if the process dies, reads expose the pending values.
                    view.update(pending=changes, revision=revision(fields, changes))
                    atomic(snapshot, view)
                    writing = True
                    run('ChangeBiosCfg', op / 'changes.xml')
                    result['output'] = 'SUM 已接受指定设置。未重启机器；请安排重启后重新读取验证。'
                else:
                    discarded = request['action'].get('discard_pending') and prior.get('pending')
                    result['output'] = 'BIOS 配置已读取。' + ('已按当前回读值清除待生效标记。' if discarded else '存在待生效或待核实的修改。' if pending else '')
                atomic(snapshot, view)
                result.update(status='succeeded', result={'pending':view['pending'], 'backup_id':operation})
        except Exception as error:
            # Once dispatched, errors do not prove firmware was unchanged.
            result.update(status='unknown' if writing else 'failed', error=str(error).replace(password, '[REDACTED]'))
        atomic(journal, result)
        return result
    
    finally:
        lock.close()


if __name__ == '__main__':
    print(json.dumps(main(json.load(sys.stdin)), ensure_ascii=False))
