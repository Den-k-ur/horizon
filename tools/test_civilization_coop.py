"""Two real fork clients: attach our running elder proof and join a fresh LAN guest.

The host is never quit or replaced. Supply the control socket of the isolated
CivilizationProof fixture produced by test_civilization_native.py --leave-running.
Only the newly created guest home is launched/relaunched by this script.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import uuid


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument('--host-socket', type=Path, required=True)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--ctl', type=Path, required=True)
    parser.add_argument('--resources', type=Path, required=True)
    parser.add_argument('--proof-dir', type=Path, required=True)
    parser.add_argument('--leave-running', action='store_true')
    args = parser.parse_args()
    proof = args.proof_dir.resolve()
    proof.mkdir(parents=True, exist_ok=True)
    host_socket = args.host_socket.resolve()
    host_home = host_socket.parent
    if not host_home.name.startswith('civilization-'):
        raise RuntimeError('Attach only a civilization-* scratch proof home, never a player world')
    guest_home = Path(tempfile.mkdtemp(prefix='civilization-guest-', dir=proof))
    guest_socket = guest_home / 'control.json'
    guest_name = 'CivGuest' + uuid.uuid4().hex[:6]
    transcript, checks = [], []
    success = False
    error = None
    guest_launched = False
    lan_opened = False
    guest_uuid = None
    port = None

    def environment(home):
        env = os.environ.copy()
        for key in ('HORIZON_HOME', 'HORIZON_LAUNCHER_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME',
                    'HORIZON_LAUNCHER_TRASH', 'HORIZON_LAUNCHER_CLIPBOARD', 'HORIZON_LAUNCHER_DESKTOP'):
            env[key] = str(home)
        env['HORIZON_NO_AUDIO'] = '1'
        env['HORIZON_LAUNCHER_NO_SELF_UPDATE'] = '1'
        return env

    def ctl(who, *words, timeout=30):
        home, socket = (host_home, host_socket) if who == 'host' else (guest_home, guest_socket)
        command = list(map(str, words))
        (proof / 'progress.json').write_text(
            json.dumps({'client': who, 'command': command}), encoding='utf-8')
        # Launched Windows grandchildren can retain pipe handles. Files prevent
        # communicate() from waiting for a game to close inherited stdout/stderr.
        with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
            process = subprocess.run(
                [str(args.ctl.resolve()), '--socket', str(socket), '--timeout', str(timeout), *command],
                cwd=home, env=environment(home), stdout=output, stderr=errors, timeout=timeout + 15)
            output.seek(0)
            errors.seek(0)
            stdout = output.read().decode('utf-8', errors='replace')
            stderr = errors.read().decode('utf-8', errors='replace')
        lines = stdout.strip().splitlines()
        if not lines:
            raise RuntimeError(f'{who}: no CLI reply to {command}: {stderr}')
        result = json.loads(lines[-1])
        transcript.append({'client': who, 'command': command, 'reply': result})
        if process.returncode or result.get('error'):
            raise RuntimeError(f'{who}: {command}: {result}')
        return result

    def until(predicate, timeout=40, message='cooperative gameplay assertion'):
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

    def campaign():
        result = ctl('host', 'civilization-state')
        assert result['enabled'] and not result.get('error'), result
        return result['campaign']

    def city():
        cities = campaign()['civilizations']
        assert len(cities) == 1, cities
        return next(iter(cities.values()))

    def counts():
        return ctl('guest', 'inventory')['counts']

    def command(who, text):
        result = ctl(who, 'command', text)
        ctl(who, 'wait', 5)
        return result

    def messages():
        return ctl('guest', 'messages', 12)['chat']

    def launch_guest():
        nonlocal guest_launched
        guest_launched = True
        # No local world or --civilization flag: the server owns the campaign.
        # Offscreen still runs the real game and renderer, including NPC mirrors.
        ctl('guest', 'launch', '--game', args.game.resolve(), '--resources', args.resources.resolve(),
            '--no-pause', '--offscreen', '--render-distance', 6, timeout=180)
        ctl('guest', 'connect', f'127.0.0.1:{port}', guest_name)
        ctl('guest', 'wait-for', 'loaded', '--timeout', 180, timeout=190)
        until(lambda: ctl('guest', 'state').get('screen') == 'playing', message='guest joined')

    try:
        initial = copy.deepcopy(campaign())
        initial_city = city()
        assert initial_city['stage'] == 'elder' and initial_city['projects'] == 3
        assert len(initial_city['quests']) == 3
        assert all(quest['completed_by'] for quest in initial_city['quests'])
        assert ctl('host', 'owned-npc-mirrors')['count'] == 3
        assert not ctl('host', 'lan', 'status')['open'], 'The attached fixture is already shared'
        campaign_files = list((host_home / 'saves').glob('*/horizon-civilization.json'))
        assert len(campaign_files) == 1, 'Expected one isolated campaign world in the host proof home'
        campaign_file = campaign_files[0]
        city_id = initial_city['id']
        initial_introduced = set(initial['introduced'])
        port = ctl('host', 'lan', 'open', 0)['port']  # Cheats for guests stay disabled.
        assert port
        lan_opened = True
        launch_guest()
        until(lambda: counts().get('minecraft:written_book', 0) == 1
              and counts().get('minecraft:paper', 0) == 1, message='guest starter kit')
        introduced = until(lambda: (set(campaign()['introduced']) - initial_introduced),
                           message='authenticated guest introduced')
        assert len(introduced) == 1, introduced
        guest_uuid = next(iter(introduced))
        command('guest', '/civ id')
        until(lambda: any(guest_uuid in line for line in messages()), message='guest UUID command reply')
        assert re.fullmatch(r'[0-9a-f]{32}', guest_uuid), guest_uuid
        guest_inventory = ctl('guest', 'inventory')
        stacks = guest_inventory['hotbar'] + guest_inventory['main']
        assert any('horizon_civ_charter' in stack.get('components', '') for stack in stacks)
        assert any('written_book_content' in stack.get('components', '') for stack in stacks)
        command('guest', '/give @s minecraft:diamond 1')
        assert counts().get('minecraft:diamond', 0) == 0, 'Guest unexpectedly has operator commands'
        check('a real non-operator LAN guest receives its own marked starter kit and authenticated UUID')

        before_denied = copy.deepcopy(campaign())
        command('guest', '/civ join ' + city_id)
        assert campaign() == before_denied, 'Uninvited player joined the civilization'
        assert guest_uuid not in city()['members']
        assert any('приглашен' in line.lower() for line in messages()), messages()
        foreign_uuid = 'f' * 32
        command('guest', '/civ invite ' + foreign_uuid)
        assert campaign() == before_denied, 'A non-member changed civilization invitations'
        check('uninvited join and foreign civilization invitation are rejected without changing the campaign')

        command('host', '/civ invite ' + guest_uuid)
        until(lambda: guest_uuid in city()['invitations'], message='founder invitation persisted')
        command('guest', '/civ join ' + city_id)
        until(lambda: guest_uuid in city()['members'], message='guest accepted invitation')
        assert guest_uuid not in city()['invitations']
        joined = copy.deepcopy(city())
        for field in ('recognition', 'projects', 'stage', 'resources', 'quests'):
            assert joined[field] == initial_city[field], field
        assert counts().get('minecraft:paper', 0) == 1, 'Joining consumed a founding charter'
        command('guest', '/civ resources')
        until(lambda: any(city_id in line and '60/60' in line for line in messages()),
              message='guest shared elder overview')
        assert any('жилых мест 6' in line for line in messages()), messages()
        check('the founder invites the guest; membership shares existing progress without granting duplicate rewards')

        before_member_invite = copy.deepcopy(campaign())
        command('guest', '/civ invite ' + foreign_uuid)
        assert campaign() == before_member_invite, 'An ordinary member invited another player'
        assert any('основатель' in line.lower() for line in messages()), messages()
        check('ordinary members cannot grant membership to a third player')

        bell = joined['bell']
        command('host', f'/tp {guest_name} {bell["x"] + 0.5} {bell["y"]} {bell["z"] - 3.5}')
        until(lambda: ctl('guest', 'owned-npc-mirrors')['count'] == 3,
              message='three NPC mirrors on the external LAN client')
        quest = joined['quests'][0]
        npcs = ctl('host', 'owned-npcs', json.dumps({
            'owner': 'civilization', 'command': {'operation': 'list'}}))['npcs']
        speaker = next(npc for npc in npcs if npc['stable_key'] == quest['npc_key'])
        position = speaker['position']
        ctl('guest', 'look-at', position['x'], position['y'] + 1, position['z'])
        ctl('guest', 'screenshot', proof / 'civilization-coop-guest.png')
        check('an external fork client renders all three independent village representatives')

        before_restart = copy.deepcopy(campaign())
        before_inventory = counts()
        ctl('host', 'save')
        assert json.loads(campaign_file.read_text(encoding='utf-8')) == before_restart
        ctl('guest', 'quit')
        guest_launched = False
        until(lambda: all(guest['name'] != guest_name for guest in ctl('host', 'lan', 'status')['guests']),
              message='guest disconnected and server saved its player data')
        time.sleep(1)
        launch_guest()
        until(lambda: counts() == before_inventory, message='restored guest inventory without duplicate starter')
        command('guest', '/civ id')
        until(lambda: any(guest_uuid in line for line in messages()), message='same UUID after guest restart')
        assert campaign() == before_restart
        assert guest_uuid in city()['members']
        assert counts().get('minecraft:written_book', 0) == 1
        assert counts().get('minecraft:paper', 0) == 1
        command('guest', '/civ resources')
        until(lambda: any(city_id in line and '60/60' in line for line in messages()),
              message='restored guest campaign access')
        until(lambda: ctl('guest', 'owned-npc-mirrors')['count'] == 3,
              message='restored LAN NPC mirrors')
        ctl('host', 'save')
        assert json.loads(campaign_file.read_text(encoding='utf-8')) == campaign()
        check('guest process restart preserves UUID, membership, inventory and NPC mirrors; host campaign is on disk')
        success = True
    except BaseException as exc:
        error = repr(exc)
        raise
    finally:
        if guest_launched and not (success and args.leave_running):
            try:
                ctl('guest', 'quit', timeout=20)
            except Exception as cleanup_error:
                transcript.append({'cleanup_guest_error': repr(cleanup_error)})
        if lan_opened and not (success and args.leave_running):
            try:
                ctl('host', 'save')
                ctl('host', 'lan', 'close')
            except Exception as cleanup_error:
                transcript.append({'cleanup_lan_error': repr(cleanup_error)})
        (proof / 'report.json').write_text(json.dumps({
            'success': success, 'error': error, 'checks': checks,
            'host_socket': str(host_socket), 'guest_home': str(guest_home),
            'guest_socket': str(guest_socket), 'guest_name': guest_name, 'guest_uuid': guest_uuid,
            'port': port, 'game': str(args.game.resolve()),
            'fixture': 'existing isolated elder village host and one fresh non-operator LAN client',
            'transcript': transcript}, ensure_ascii=False, indent=2), encoding='utf-8')
    print(str(proof / 'report.json'), flush=True)


if __name__ == '__main__':
    main()
