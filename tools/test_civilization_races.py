"""Real book clicks, race starts, compass wire data, modifiers and two-client persistence."""
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
    for name in ('game','ctl','resources','proof-dir'):
        parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--skip-guest',action='store_true')
    args = parser.parse_args()
    proof = args.proof_dir.resolve(); proof.mkdir(parents=True,exist_ok=True)
    homes = {who:Path(tempfile.mkdtemp(prefix='civilization-race-'+who+'-',dir=proof)) for who in ('host','guest')}
    sockets = {who:home/'control.json' for who,home in homes.items()}
    running = set(); transcript=[]; checks=[]; success=False; error=None
    guest_name='RaceGuest'+uuid.uuid4().hex[:6]

    def env(who):
        result=os.environ.copy()
        for key in ('HORIZON_HOME','HORIZON_LAUNCHER_HOME','XDG_DATA_HOME','XDG_CONFIG_HOME',
                    'HORIZON_LAUNCHER_TRASH','HORIZON_LAUNCHER_CLIPBOARD','HORIZON_LAUNCHER_DESKTOP'):
            result[key]=str(homes[who])
        result.update(HORIZON_NO_AUDIO='1',HORIZON_LAUNCHER_NO_SELF_UPDATE='1')
        return result

    def ctl(who,*words,timeout=30):
        words=list(map(str,words))
        (proof/'progress.json').write_text(json.dumps({'client':who,'command':words}),encoding='utf-8')
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            process=subprocess.run([str(args.ctl.resolve()),'--socket',str(sockets[who]),'--timeout',str(timeout),*words],
                env=env(who),cwd=homes[who],stdout=out,stderr=err,timeout=timeout+15)
            out.seek(0); err.seek(0)
            stdout=out.read().decode('utf-8',errors='replace'); stderr=err.read().decode('utf-8',errors='replace')
        lines=stdout.strip().splitlines()
        if not lines: raise RuntimeError((who,words,stderr))
        result=json.loads(lines[-1]); transcript.append({'client':who,'command':words,'reply':result})
        if process.returncode or result.get('error'): raise RuntimeError((who,words,result))
        return result

    def until(predicate,message,timeout=180):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            result=predicate()
            if result: return result
            time.sleep(0.2)
        raise AssertionError('Timed out: '+message)

    def check(message):
        checks.append(message); print('PASS '+message,flush=True)

    def command(who,text):
        result=ctl(who,'command',text); ctl(who,'wait',5); return result

    def campaign():
        result=ctl('host','civilization-state')
        assert result['enabled'] and not result.get('error'),result
        data=result['campaign']
        for c in data['civilizations'].values():
            a=c.get('governance',{}).get('assessment')
            if a:a.pop('tick',None);a.pop('sample_tick',None)
            population=c.get('population')
            if population:
                for field in ('last_tick','active_ticks','meal_in'):population.pop(field,None)
        return data

    def counts(who): return ctl(who,'inventory')['counts']

    def start_host():
        running.add('host')
        ctl('host','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),
            '--world','RaceProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)

    def start_guest(port):
        running.add('guest')
        ctl('guest','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),
            '--offscreen','--no-pause','--render-distance',6,timeout=180)
        ctl('guest','connect',f'127.0.0.1:{port}',guest_name)
        ctl('guest','wait-for','loaded','--timeout',180,timeout=190)

    def book_choose(who,race):
        ctl(who,'select','minecraft:written_book'); ctl(who,'use-item')
        screen=until(lambda:(r if (r:=ctl(who,'screen'))['screen']=='book' else None),'book open')
        done=next(button for button in screen['buttons'] if button['label']=='Done')
        left=done['x']+4
        ctl(who,'screenshot',proof/f'{who}-menu.png')
        row=4 if race=='desert-cat' else 2
        ctl(who,'click-at',left+43,32+row*9+3); time.sleep(0.2)
        ctl(who,'screenshot',proof/f'{who}-race.png')
        row=7 if race=='desert-cat' else 6
        ctl(who,'click-at',left+43,32+row*9+3); time.sleep(0.2)
        screen=ctl(who,'screen')
        if screen['screen']!='playing':
            run=next((b for b in screen.get('buttons',[]) if b['label'] in ('Run','Yes','Run Command') and b['enabled']),None)
            if run: ctl(who,'click-button',run['label'])

    def compass(who,choice):
        inv=ctl(who,'inventory')
        stack=next(s for s in inv['hotbar']+inv['main']+[inv.get('offhand',{})]
                   if s and s.get('item')=='minecraft:compass' and 'horizon_civ_compass' in s.get('components',''))
        data=stack['components']
        assert 'lodestone_tracker' in data and 'minecraft:overworld' in data,data
        match=re.search(r'pos\s*:\s*\[I;([^\]]+)\]',data)
        assert match,data
        values=[int(v.strip()) for v in match[1].split(',')]
        b=choice['start']['settlement']
        assert values==[b['x'],b['y'],b['z']],data
        return data

    def speed():
        line=command('host','/attribute @s minecraft:movement_speed get')['chat'][-1]
        return float(re.findall(r'[-+]?\d*\.?\d+(?:[eE][-+]?\d+)?',line)[-1])

    try:
        start_host()
        until(lambda:counts('host').get('minecraft:compass')==1,'three-item starter')
        initial=campaign(); assert initial['version']==8 and initial['choices']=={}
        host_id=initial['introduced'][0]
        identity=(homes['host']/'horizon-player.txt').read_bytes()
        origin=ctl('host','state')['player']['position']
        command('host',f'/worldborder center {origin[0]} {origin[2]}')
        command('host','/worldborder set 32')
        book_choose('host','desert-cat')
        refused=until(lambda:ctl('host','civilization-state').get('start_errors',{}).get(host_id),
                      'race start outside world border rejected')
        assert refused in ('horizon.civilization.race.invalid_start','horizon.civilization.race.not_found'),refused
        messages=ctl('host','messages',12)
        assert any(isinstance(text,dict) and text.get('translate')==refused
                   for text in messages['styled']),messages
        assert campaign()['choices']=={}
        after=ctl('host','state')['player']['position']
        assert abs(after[0]-origin[0])<1 and abs(after[2]-origin[2])<1
        command('host','/worldborder set 59999968')
        command('host','/worldborder center 0 0')
        check('a destination beyond the world border is refused without committing a race or moving the player')
        book_choose('host','desert-cat')
        choice=until(lambda:campaign()['choices'].get(host_id),'book-selected race')
        assert choice['race']=='desert-cat'
        check('clicking the real book menu selects Desert Cat on the authoritative server')
        choice=until(lambda:(c if not (c:=campaign()['choices'][host_id])['arriving'] else None),'safe desert arrival')
        position=ctl('host','state')['player']['position']
        start=choice['start']['position']
        assert abs(position[0]-start[0])<12 and abs(position[2]-start[2])<12,position
        reply=command('host','/execute if biome ~ ~ ~ minecraft:desert run say CIV_DESERT_OK')
        assert any('CIV_DESERT_OK' in text for text in reply['chat']),reply
        assert (homes['host']/'horizon-player.txt').read_bytes()==identity
        compass('host',choice)
        ctl('host','screenshot',proof/'desert-arrival.png')
        check('safe arrival uses actual desert blocks and the inventory compass tracks the server-selected settlement')
        before=copy.deepcopy(campaign()); before_position=ctl('host','state')['player']['position']
        command('host','/civ race choose human')
        assert campaign()==before
        after_position=ctl('host','state')['player']['position']
        assert abs(after_position[0]-before_position[0])<1 and abs(after_position[2]-before_position[2])<1
        check('repeated selection cannot change the saved race, target or identity')
        if args.skip_guest:
            success=True; return

        x,_,z=map(lambda v:int(v//1),position)
        ctl('host','fill',x-2,198,z-2,x+2,199,z+2,'minecraft:sandstone')
        ctl('host','fill',x-2,200,z-2,x+2,204,z+2,'minecraft:air')
        ctl('host','setblock',x,199,z,'minecraft:sand')
        ctl('host','tp',x+0.5,200,z+0.5); ctl('host','wait',30)
        assert abs(ctl('host','state')['player']['position'][1]-200)<0.1
        assert abs(speed()-0.12)<0.00001
        command('host',f'/fillbiome {x-4} 196 {z-4} {x+4} 207 {z+4} minecraft:snowy_plains')
        ctl('host','wait',30)
        assert abs(speed()-0.085)<0.00001
        command('host',f'/fillbiome {x-4} 196 {z-4} {x+4} 207 {z+4} minecraft:desert')
        ctl('host','setblock',x,199,z,'minecraft:stone'); ctl('host','wait',30)
        assert abs(speed()-0.11)<0.00001
        check('real server attributes apply sand speed, cold penalty and ordinary movement as independent race effects')

        port=ctl('host','lan','open',0)['port']; start_guest(port)
        until(lambda:counts('guest').get('minecraft:compass')==1,'guest starter')
        guest_id=next(p for p in campaign()['introduced'] if p!=host_id)
        before=copy.deepcopy(campaign())
        command('guest','/civ race choose human '+host_id)
        assert campaign()==before
        book_choose('guest','human')
        until(lambda:guest_id in campaign()['choices'],'guest choice')
        human=until(lambda:(c if not (c:=campaign()['choices'][guest_id])['arriving'] else None),'guest plains arrival')
        assert human['race']=='human'
        compass('guest',human)
        assert campaign()['choices'][host_id]==choice
        command('guest','/give @s minecraft:diamond 1')
        assert counts('guest').get('minecraft:diamond',0)==0
        check('a real non-operator guest chooses Human through the book without changing another player choice')

        before=copy.deepcopy(campaign())
        command('host',f'/gamemode survival {guest_name}'); command('host',f'/kill {guest_name}')
        until(lambda:ctl('guest','screen')['screen']=='dead','death')
        ctl('guest','respawn'); until(lambda:ctl('guest','screen')['screen']=='playing','respawn')
        assert campaign()==before
        command('guest','/civ compass'); command('guest','/civ compass')
        until(lambda:counts('guest').get('minecraft:compass')==1,'single recovered compass')
        compass('guest',human)
        command('guest','/civ guide'); command('guest','/civ guide')
        until(lambda:counts('guest').get('minecraft:written_book')==1,'single recovered guide')
        assert campaign()==before
        check('death retains race and recovery grants one correctly targeted compass and one guide')

        expected=copy.deepcopy(campaign())
        ctl('guest','quit'); running.remove('guest'); ctl('host','save'); ctl('host','quit'); running.remove('host')
        time.sleep(1); start_host()
        until(lambda:campaign()==expected,'saved choices after host restart')
        assert (homes['host']/'horizon-player.txt').read_bytes()==identity
        compass('host',choice)
        port=ctl('host','lan','open',0)['port']; start_guest(port)
        until(lambda:counts('guest').get('minecraft:compass')==1,'guest inventory after restart')
        compass('guest',human)
        assert campaign()==expected
        check('host and guest restarts preserve both race choices, targets, identity and recovered inventory')
        success=True
    except BaseException as exc:
        error=repr(exc); raise
    finally:
        for who in ('guest','host'):
            if who in running:
                try: ctl(who,'quit',timeout=15)
                except Exception as exc: transcript.append({'cleanup':who,'error':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,
            'homes':{k:str(v) for k,v in homes.items()},'sockets':{k:str(v) for k,v in sockets.items()},
            'game':str(args.game.resolve()),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf-8')


if __name__=='__main__': main()
