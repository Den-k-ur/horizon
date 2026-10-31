"""Prove Civilization's first chapter in a fresh integrated-server world."""
import argparse
import copy
import json
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
    args = parser.parse_args()
    proof = args.proof_dir.resolve()
    proof.mkdir(parents=True, exist_ok=True)
    home = Path(tempfile.mkdtemp(prefix='civilization-', dir=proof))
    socket = home / 'control.json'
    env = os.environ.copy()
    for key in ('HORIZON_HOME', 'HORIZON_LAUNCHER_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
                'HORIZON_LAUNCHER_TRASH', 'HORIZON_LAUNCHER_CLIPBOARD', 'HORIZON_LAUNCHER_DESKTOP'):
        env[key] = str(home)
    env['HORIZON_NO_AUDIO'] = '1'
    env['HORIZON_LAUNCHER_NO_SELF_UPDATE'] = '1'
    transcript = []
    checks = []
    error = None
    success = False
    launched = False

    def ctl(*words, timeout=30):
        command = list(map(str, words))
        (proof / 'progress.json').write_text(json.dumps({'command': command}), encoding='utf-8')
        # Windows grandchildren can retain pipe handles. Regular temporary files
        # avoid waiting for the launched game to close a CLI's inherited stdout.
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            process = subprocess.run(
                [str(args.ctl.resolve()), '--socket', str(socket), '--timeout', str(timeout), *command],
                env=env, cwd=home, stdout=output, stderr=errors, timeout=timeout + 15)
            output.seek(0)
            errors.seek(0)
            stdout = output.read().decode('utf-8', errors='replace')
            stderr = errors.read().decode('utf-8', errors='replace')
        lines = stdout.strip().splitlines()
        if not lines:
            raise RuntimeError(f'No CLI reply to {command}: {stderr}')
        result = json.loads(lines[-1])
        transcript.append({'command': command, 'reply': result})
        if process.returncode or result.get('error'):
            raise RuntimeError(f'{command}: {result}')
        return result

    def until(predicate, timeout=30, message='gameplay assertion'):
        deadline = time.monotonic() + timeout
        while True:
            value = predicate()
            if value:
                return value
            if time.monotonic() >= deadline:
                raise AssertionError(f'Timed out waiting for {message}')
            time.sleep(0.2)

    def check(name):
        checks.append(name)
        print('PASS ' + name, flush=True)

    def launch(enable):
        nonlocal launched
        words = ['launch', '--game', args.game.resolve(), '--world', 'CivilizationProof',
                 '--seed', '42', '--creative', '--wait', '--resources', args.resources.resolve(),
                 '--no-pause', '--render-distance', '6']
        if enable:
            words.append('--civilization')
        # Even if readiness fails, cleanup targets only this newly allocated home.
        launched = True
        ctl(*words, timeout=180)

    def campaign():
        result = ctl('civilization-state')
        assert result['enabled'], result
        assert not result.get('error'), result
        return result['campaign']

    def city():
        cities = campaign()['civilizations']
        assert len(cities) == 1, cities
        return next(iter(cities.values()))

    def counts():
        return ctl('inventory')['counts']

    def command(text):
        result = ctl('command', text)
        ctl('wait', 3)
        return result

    def npcs():
        return ctl('owned-npcs', json.dumps({'owner': 'civilization',
                    'command': {'operation': 'list'}}, separators=(',', ':')))['npcs']

    def near(quest):
        npc = next(n for n in npcs() if n['stable_key'] == quest['npc_key'])
        position = npc['position']
        ctl('tp', position['x'] + 1.5, position['y'], position['z'])
        ctl('wait', 3)
        ctl('look-at', position['x'], position['y'] + 1.0, position['z'])

    try:
        launch(True)
        until(lambda: counts().get('minecraft:written_book', 0) == 1
              and counts().get('minecraft:paper', 0) == 1, message='starter kit')
        starter = ctl('inventory')
        stacks = starter['hotbar'] + starter['main']
        paper = next(s for s in stacks if s.get('item') == 'minecraft:paper')
        book = next(s for s in stacks if s.get('item') == 'minecraft:written_book')
        assert 'horizon_civ_charter' in paper.get('components', ''), paper
        assert 'written_book_content' in book.get('components', ''), book
        assert len(campaign()['introduced']) == 1
        ctl('select', 'minecraft:written_book')
        ctl('wait', 3)
        ctl('use-item')
        until(lambda: ctl('screen')['screen'] == 'book', message='readable starter book screen')
        ctl('screenshot', proof / 'civilization-guide.png')
        ctl('click-button', 'Done')
        until(lambda: ctl('screen')['screen'] == 'playing', message='book closed')
        check('new player receives a readable guide and a marked founding charter once')

        # Fixture only: an isolated player-built village with two complete beds
        # and a stationary ambient villager. No generated-village claim is made.
        ctl('fill', -12, 100, -12, 12, 106, 12, 'minecraft:air')
        ctl('fill', -12, 99, -12, 12, 99, 12, 'minecraft:stone')
        ctl('setblock', 0, 100, 0, 'minecraft:bell[attachment=floor,facing=north,powered=false]')
        for z in (3, 5):
            ctl('setblock', 3, 100, z, 'minecraft:red_bed[facing=east,part=foot,occupied=false]')
            ctl('setblock', 4, 100, z, 'minecraft:red_bed[facing=east,part=head,occupied=false]')
        command('/summon minecraft:villager 5.5 100 1.5 {NoAI:1b,PersistenceRequired:1b}')
        ctl('tp', 0.5, 100, -2.5)
        ctl('select', 'minecraft:paper')
        ctl('wait', 3)
        ctl('interact', 0, 100, 0, 'north')
        until(lambda: len(campaign()['civilizations']) == 1, message='charter used on village bell')
        founded = city()
        assert founded['stage'] == 'recognition'
        assert founded['recognition'] == founded['projects'] == 0
        until(lambda: counts().get('minecraft:paper', 0) == 0, message='charter consumed')
        check('real right-click on a village bell consumes the charter and starts recognition')

        until(lambda: len(npcs()) == 3 and all(n['presence'] == 'loaded' for n in npcs()),
              message='three independent village representatives')
        until(lambda: ctl('owned-npc-mirrors')['count'] == 3, message='three visible NPC mirrors')
        assert all(n['definition'] == 'civilization:representative' for n in npcs())
        check('three stable independent NPC representatives are authoritative and rendered')

        quarry = next(q for q in city()['quests'] if q['id'] == 'quarry')
        near(quarry)
        command('/civ talk quarry')
        ctl('give', 'minecraft:oak_log', 16)
        until(lambda: counts().get('minecraft:oak_log', 0) == 16, message='fixture logs arrived')
        before_failed = counts()
        command('/civ deliver quarry')
        assert city()['recognition'] == 0
        assert counts() == before_failed, 'Insufficient delivery consumed a partial cost'
        check('insufficient delivery changes neither project progress nor any inventory items')

        ctl('give', 'minecraft:cobblestone', 32)
        until(lambda: counts().get('minecraft:cobblestone', 0) == 32)
        command('/civ deliver quarry')
        assert city()['recognition'] == 20 and city()['projects'] == 1
        until(lambda: counts().get('minecraft:oak_log', 0) == 0
              and counts().get('minecraft:cobblestone', 0) == 0)
        assert city()['resources']['wood'] == 0, 'Spent rebuilding materials became duplicate treasury resources'
        check('full delivery consumes real materials once and awards one project and twenty recognition')

        ctl('give', 'minecraft:oak_log', 16)
        ctl('give', 'minecraft:cobblestone', 32)
        until(lambda: counts().get('minecraft:cobblestone', 0) == 32)
        before_duplicate = counts()
        command('/civ deliver quarry')
        assert city()['recognition'] == 20 and city()['projects'] == 1
        assert counts() == before_duplicate
        check('a repeated completed quest grants no reward and consumes no second payment')

        for quest_id, item, count in [('granary', 'minecraft:wheat', 24),
                                     ('housing', 'minecraft:oak_planks', 32)]:
            quest = next(q for q in city()['quests'] if q['id'] == quest_id)
            near(quest)
            ctl('give', item, count)
            until(lambda: counts().get(item, 0) == count)
            command('/civ deliver ' + quest_id)
            until(lambda: counts().get(item, 0) == 0)
        elder = city()
        assert elder['stage'] == 'elder' and elder['recognition'] == 60 and elder['projects'] == 3
        assert all(q['completed_by'] for q in elder['quests'])
        assert elder['resources'] == {'wood': 0, 'food': 0, 'housing': 6}
        command('/civ resources')
        ctl('screenshot', proof / 'civilization-elder.png')
        check('three completed projects unlock elder status and the shared resource overview')

        before_restart = copy.deepcopy(campaign())
        before_inventory = counts()
        before_npc_ids = sorted(n['stable_key'] for n in npcs())
        ctl('save')
        ctl('quit')
        launched = False
        time.sleep(1)
        launch(False)  # Persistent campaign files enable the world without the flag.
        until(lambda: ctl('owned-npc-mirrors')['count'] == 3, message='restored NPC mirrors')
        assert campaign() == before_restart
        until(lambda: counts() == before_inventory, message='restored inventory without duplicate starter')
        assert sorted(n['stable_key'] for n in npcs()) == before_npc_ids
        assert counts().get('minecraft:written_book', 0) == 1 and counts().get('minecraft:paper', 0) == 0
        command('/civ resources')
        ctl('screenshot', proof / 'civilization-restored.png')
        check('restart restores the campaign, inventory and three NPC identities without duplicate grants')
        success = True
    except BaseException as exc:
        error = repr(exc)
        raise
    finally:
        if launched and not args.leave_running:
            try:
                ctl('save', timeout=30)
                ctl('quit', timeout=15)
            except (RuntimeError, subprocess.TimeoutExpired):
                pass
        (proof / 'report.json').write_text(json.dumps({
            'success': success, 'error': error, 'checks': checks, 'home': str(home),
            'socket': str(socket), 'game': str(args.game.resolve()),
            'fixture': 'player-built village, two complete beds, one stationary ambient villager',
            'transcript': transcript}, ensure_ascii=False, indent=2), encoding='utf-8')
    print(str(proof / 'report.json'), flush=True)


if __name__ == '__main__':
    main()
