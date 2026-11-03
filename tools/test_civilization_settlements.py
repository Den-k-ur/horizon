"""Real own-settlement construction, founding and preservation; never opens a user world."""
import argparse
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time


def main():
    parser=argparse.ArgumentParser(__doc__)
    for name in ('game','ctl','resources','proof-dir'):
        parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args();proof=args.proof_dir.resolve();proof.mkdir(parents=True,exist_ok=True)
    home=Path(tempfile.mkdtemp(prefix='civilization-towns-',dir=proof));socket=home/'control.json'
    env=os.environ.copy()
    for key in ('HORIZON_HOME','HORIZON_LAUNCHER_HOME','XDG_DATA_HOME','XDG_CONFIG_HOME','HORIZON_LAUNCHER_TRASH','HORIZON_LAUNCHER_CLIPBOARD','HORIZON_LAUNCHER_DESKTOP'):
        env[key]=str(home)
    env.update(HORIZON_NO_AUDIO='1',HORIZON_LAUNCHER_NO_SELF_UPDATE='1')
    transcript=[];checks=[];success=False;error=None;running=False

    def ctl(*words,timeout=30):
        words=list(map(str,words));(proof/'progress.json').write_text(json.dumps(words),encoding='utf8')
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            p=subprocess.run([str(args.ctl.resolve()),'--socket',str(socket),'--timeout',str(timeout),*words],env=env,cwd=home,stdout=out,stderr=err,timeout=timeout+15)
            out.seek(0);err.seek(0);stdout=out.read().decode('utf8',errors='replace');stderr=err.read().decode('utf8',errors='replace')
        if not stdout.strip():raise RuntimeError((words,stderr))
        data=json.loads(stdout.strip().splitlines()[-1]);transcript.append({'command':words,'reply':data})
        if p.returncode or data.get('error'):raise RuntimeError((words,data))
        return data

    def until(predicate,message,timeout=240):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            result=predicate()
            if result:return result
            time.sleep(.2)
        raise AssertionError('Timed out: '+message)

    def check(name):checks.append(name);print('PASS '+name,flush=True)
    def campaign():
        state=ctl('civilization-state');assert state['enabled'] and not state.get('error'),state
        data=state['campaign']
        for c in data['civilizations'].values():
            a=c.get('governance',{}).get('assessment')
            if a:a.pop('tick',None);a.pop('sample_tick',None)
            population=c.get('population')
            if population:
                for field in ('last_tick','active_ticks','meal_in'):population.pop(field,None)
        return data
    def command(text):
        r=ctl('command',text);ctl('wait',5);return r
    def launch():
        nonlocal running
        running=True
        ctl('launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world','TownsProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)
    def npcs(town):return ctl('owned-npcs',json.dumps({'owner':town['npc_owner'],'command':{'operation':'list'}}))['npcs']
    def block(x,y,z):return ctl('getblock',x,y,z)

    try:
        launch();until(lambda:len(campaign()['introduced'])==1,'starter');player=campaign()['introduced'][0]
        command('/civ race choose desert-cat')
        def selected():
            result=ctl('civilization-state')
            assert not result.get('start_errors',{}).get(player),result
            return result['campaign']['choices'].get(player)
        choice=until(selected,'own settlement selection')
        assert choice['start']['structure']=='civilization:settlement'
        town_id=choice['settlement_id'];assert town_id.startswith('town-')
        pending=campaign()['settlements'][town_id]
        assert pending['status']=='building',pending
        ctl('save');ctl('quit');running=False;time.sleep(1);launch()
        check('an unfinished settlement survives a real server save and restart')
        until(lambda:campaign()['settlements'][town_id]['status']=='ready','camp buildings ready')
        until(lambda:not campaign()['choices'][player]['arriving'],'arrival after construction')
        town=campaign()['settlements'][town_id];b=town['bell'];x,y,z=b['x'],b['y'],b['z']
        assert town['kind']=='generated' and town['claimed_by'] is None and town['building_started']
        assert 'minecraft:bell' in json.dumps(block(x,y,z))
        for dx,dz in [(-7,-6),(5,-6),(-1,6)]:assert 'minecraft:red_bed' in json.dumps(block(x+dx,y,z+dz))
        until(lambda:len(npcs(town))==3 and all(n['presence']=='loaded' for n in npcs(town)),'three own residents')
        identities=copy.deepcopy(npcs(town));assert {n['stable_key'] for n in identities}==set(town['residents'])
        check('race creates an original camp with three beds, bell and three persistent independent residents')
        command('/kill @e[type=minecraft:villager]')
        ctl('tp',x+.5,y,z-2.5);ctl('look-at',x+.5,y+.5,z+.5);ctl('wait',3)
        ctl('select','minecraft:paper');ctl('wait',3);ctl('interact',x,y,z,'north')
        city=until(lambda:next(iter(campaign()['civilizations'].values()),None),'bell charter founding')
        assert city['founder']==player and city['leader']==player and city['stage']=='recognition'
        assert (city['recognition'],city['projects'])==(0,0) and city['settlement_id']==town_id
        assert campaign()['settlements'][town_id]['status']=='potential-capital'
        assert not ctl('inventory')['counts'].get('minecraft:paper',0)
        assert {n['stable_key'] for n in npcs(town)}==set(town['residents'])
        assert {(n['owner'],n['stable_key'],n['city']) for n in npcs(town)}=={(n['owner'],n['stable_key'],n['city']) for n in identities}
        check('using the charter on the bell founds a potential capital without vanilla villagers or automatic Elder rank')
        until(lambda:campaign()['civilizations'][city['id']]['governance']['assessment'] is not None,'initial settlement survey')
        before=copy.deepcopy(campaign());command('/civ found Again');assert campaign()==before
        ctl('setblock',x+9,y,z+9,'minecraft:diamond_block')
        ctl('wait',45);assert 'minecraft:diamond_block' in json.dumps(block(x+9,y,z+9))
        check('repeat founding does not consume or reward again and a finished camp is never rebuilt')
        ctl('screenshot',proof/'own-settlement.png');ctl('save');ctl('quit');running=False;time.sleep(1)
        launch();until(lambda:campaign()==before,'campaign restart')
        ctl('tp',x+.5,y,z-2.5);ctl('wait',40)
        assert 'minecraft:diamond_block' in json.dumps(block(x+9,y,z+9))
        restored=npcs(town);assert {(n['owner'],n['stable_key'],n['city']) for n in restored}=={(n['owner'],n['stable_key'],n['city']) for n in identities}
        assert len(restored)==3
        inv=ctl('inventory');stack=next(s for s in inv['hotbar']+inv['main'] if s.get('item')=='minecraft:compass')
        coords=re.search(r'pos\s*:\s*\[I;([^\]]+)\]',stack['components'])
        assert [int(v.strip()) for v in coords[1].split(',')]==[x,y,z]
        check('restart retains capital, citizen identities, compass target and player building edits')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        if running:
            try:ctl('quit',timeout=20)
            except Exception as exc:transcript.append({'cleanup':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,'home':str(home),'socket':str(socket),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
