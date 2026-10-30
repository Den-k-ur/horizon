"""Exercise our fork's owned-NPC API on a real integrated server, never player data."""
import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--ctl', type=Path, required=True)
    parser.add_argument('--resources', type=Path, required=True)
    parser.add_argument('--proof-dir', type=Path, required=True)
    parser.add_argument('--leave-running', action='store_true')
    parser.add_argument('--reuse-home', type=Path)
    args = parser.parse_args()
    proof = args.proof_dir.resolve()
    proof.mkdir(parents=True, exist_ok=True)
    home = args.reuse_home.resolve() if args.reuse_home else Path(tempfile.mkdtemp(prefix='owned-npcs-', dir=proof))
    if home.parent != proof or not home.name.startswith('owned-npcs-'):
        raise RuntimeError('Only a test home directly inside proof-dir may be reused')
    socket = home / 'control.json'
    env = os.environ.copy()
    for key in ('HORIZON_HOME', 'HORIZON_LAUNCHER_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
                'HORIZON_LAUNCHER_TRASH', 'HORIZON_LAUNCHER_CLIPBOARD', 'HORIZON_LAUNCHER_DESKTOP'):
        env[key] = str(home)
    env['HORIZON_NO_AUDIO'] = '1'
    env['HORIZON_LAUNCHER_NO_SELF_UPDATE'] = '1'
    transcript = []
    checks = []

    def ctl(*words, timeout=30):
        (proof / 'progress.json').write_text(json.dumps({'command': list(map(str, words))}), encoding='utf-8')
        # A launched Windows game can inherit a CLI's pipe handles: communicate()
        # would then wait for EOF until the game exits. Capture to regular files.
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            process = subprocess.run([str(args.ctl.resolve()), '--socket', str(socket), '--timeout', str(timeout),
                                      *map(str, words)], env=env, cwd=home, stdout=output, stderr=errors,
                                     timeout=timeout + 15)
            output.seek(0)
            errors.seek(0)
            stdout = output.read().decode('utf-8', errors='replace')
            stderr = errors.read().decode('utf-8', errors='replace')
        lines = stdout.strip().splitlines()
        if not lines:
            raise RuntimeError(f'No CLI reply to {words}: {stderr}')
        result = json.loads(lines[-1])
        transcript.append({'command': list(map(str, words)), 'reply': result})
        if process.returncode or 'error' in result:
            raise RuntimeError(f'{words}: {result}')
        return result

    def rpc(operation, **values):
        request = {'owner': 'civilization', 'command': {'operation': operation, **values}}
        return ctl('owned-npcs', json.dumps(request, separators=(',', ':')))

    def state():
        return rpc('state', key='citizen_1')['npc']

    def until(predicate, timeout=25):
        started = time.monotonic()
        while True:
            value = predicate()
            if value:
                return value
            if time.monotonic() - started > timeout:
                raise AssertionError('Timed out waiting for the owned NPC assertion')
            time.sleep(0.2)

    def launch():
        ctl('launch', '--game', args.game.resolve(), '--world', 'OwnedNpcApiProof', '--seed', '42',
            '--creative', '--wait', '--resources', args.resources.resolve(), '--no-pause',
            '--render-distance', '6', timeout=180)

    def check(name):
        checks.append(name)
        print('PASS ' + name, flush=True)

    def move(target):
        rpc('set-order', key='citizen_1', order={'kind': 'move-to', 'target': target, 'arrival_radius': 0.6})
        samples = []

        def arrived():
            npc = state()
            samples.append(npc['position'])
            return npc if npc['order_status'] == 'complete' else None

        result = until(arrived)
        assert math.dist(list(result['position'].values()), list(target.values())) <= 0.601
        return result, samples

    success = False
    try:
        if not args.reuse_home:
            launch()
        ctl('fill', -8, 100, -8, 20, 105, 20, 'minecraft:air')
        ctl('fill', -8, 99, -8, 20, 99, 20, 'minecraft:stone')
        ctl('fill', 4, 100, -2, 4, 103, 2, 'minecraft:stone')
        ctl('tp', 0.5, 100, -5.5)
        request = {'stable_key': 'citizen_1', 'definition': 'civilization:worker', 'city': 'city_1',
                   'profession': 'wood', 'appearance': {'model': 'minecraft:entity/villager',
                   'texture': 'minecraft:entity/villager/villager'}, 'dimension': 'minecraft:overworld',
                   'spawn': {'x': 0.5, 'y': 100.0, 'z': 0.5}}
        rpc('ensure', npc=request)
        until(lambda: state()['presence'] == 'loaded')
        until(lambda: ctl('owned-npc-mirrors')['count'] == 1)
        check('server creation is rendered by the separate client mirror')
        result, samples = move({'x': 10.5, 'y': 100.0, 'z': 0.5})
        assert any(abs(p['z']) > 2.3 for p in samples), 'No movement around the obstacle observed'
        assert len({(p['x'], p['z']) for p in samples}) > 5
        check('native A* walks around an obstacle, without teleporting')
        rpc('set-order', key='citizen_1', order={'kind': 'work', 'action': 'gather_wood', 'seconds': 2.0})
        until(lambda: state()['order_status'] == 'complete')
        assert state()['work_seconds'] >= 2.0
        check('explicit work action completes through the server timer')
        move(request['spawn'])
        check('return to the store completes')
        rpc('set-order', key='citizen_1', order={'kind': 'move-to', 'target': {'x': 16.5, 'y': 100.0, 'z': 3.5}, 'arrival_radius': 0.6})
        ctl('wait', 5)
        rpc('cancel', key='citizen_1')
        cancelled = state()
        ctl('wait', 30)
        assert state()['order_status'] == 'cancelled'
        assert math.dist(list(cancelled['position'].values()), list(state()['position'].values())) < 0.5
        rpc('set-profession', key='citizen_1', profession='builder')
        assert state()['profession'] == 'builder'
        rpc('ensure', npc=request)
        assert len(rpc('list')['npcs']) == 1
        check('cancel/reassign preserves identity and ensure does not duplicate')
        ctl('look-at', state()['position']['x'], 101, state()['position']['z'])
        ctl('wait', 3)
        ctl('screenshot', proof / 'owned-npc.png')
        before_restart = state()
        ctl('save')
        ctl('quit')
        time.sleep(1)
        launch()
        until(lambda: state()['presence'] == 'loaded')
        until(lambda: ctl('owned-npc-mirrors')['count'] == 1)
        restored = state()
        assert len(rpc('list')['npcs']) == 1
        for field in ('owner', 'stable_key', 'profession', 'order_revision', 'order_status'):
            assert restored[field] == before_restart[field], field
        assert math.dist(list(restored['position'].values()), list(before_restart['position'].values())) < 0.5
        check('world restart restores position/identity/order without a duplicate')
        rpc('despawn', key='citizen_1')
        until(lambda: ctl('owned-npc-mirrors')['count'] == 0)
        assert state()['presence'] == 'despawned'
        check('despawn removes the visible mirror and keeps the identity tombstone')
        if args.leave_running:
            request['stable_key'] = 'demo_worker'
            request['profession'] = 'builder'
            rpc('ensure', npc=request)
            until(lambda: ctl('owned-npc-mirrors')['count'] == 1)
            ctl('look-at', 0.5, 101, 0.5)
        success = True
    finally:
        (proof / 'report.json').write_text(json.dumps({'success': success, 'checks': checks,
            'home': str(home), 'socket': str(socket), 'game': str(args.game.resolve()),
            'transcript': transcript}, ensure_ascii=False, indent=2), encoding='utf-8')
        if not (args.leave_running and success):
            try:
                ctl('quit', timeout=15)
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
    print(str(proof / 'report.json'), flush=True)


if __name__ == '__main__':
    main()
