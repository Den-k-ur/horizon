"""Upgrade a copy of a real Civilization proof world; source stays read-only.

Schema-v8 fixtures should have no running work or imminent meal: the upgrade
must preserve their material, assignment and progress accounts exactly.
"""
import argparse
import copy
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


def main():
    parser=argparse.ArgumentParser(__doc__)
    for name in ('game','ctl','resources','proof-dir','legacy-home'):
        parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args();source=args.legacy_home.resolve()
    assert source.name.startswith('civilization-'),source
    worlds=list((source/'saves').iterdir());assert len(worlds)==1,worlds
    old_world=worlds[0];original=(old_world/'horizon-civilization.json').read_bytes();old=json.loads(original)
    assert old['version'] in (1,2,3,4,5,6,7,8) and old['civilizations']
    source_npcs=json.loads((old_world/'horizon-owned-npcs.json').read_text(encoding='utf8'))
    proof=args.proof_dir.resolve();proof.mkdir(parents=True,exist_ok=True)
    home=Path(tempfile.mkdtemp(prefix='civilization-legacy-',dir=proof));world=home/'saves'/old_world.name
    shutil.copytree(old_world,world);shutil.copy2(source/'horizon-player.txt',home/'horizon-player.txt')
    socket=home/'control.json';transcript=[];checks=[];success=False;error=None;running=False
    env=os.environ.copy()
    for key in ('HORIZON_HOME','HORIZON_LAUNCHER_HOME','XDG_DATA_HOME','XDG_CONFIG_HOME','HORIZON_LAUNCHER_TRASH','HORIZON_LAUNCHER_CLIPBOARD','HORIZON_LAUNCHER_DESKTOP'):env[key]=str(home)
    env.update(HORIZON_NO_AUDIO='1',HORIZON_LAUNCHER_NO_SELF_UPDATE='1')

    def ctl(*words,timeout=30):
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            p=subprocess.run([str(args.ctl.resolve()),'--socket',str(socket),'--timeout',str(timeout),*map(str,words)],env=env,cwd=home,stdout=out,stderr=err,timeout=timeout+15)
            out.seek(0);err.seek(0);stdout=out.read().decode('utf8',errors='replace');stderr=err.read().decode('utf8',errors='replace')
        if not stdout.strip():raise RuntimeError((words,stderr))
        r=json.loads(stdout.strip().splitlines()[-1]);transcript.append({'command':list(map(str,words)),'reply':r})
        if p.returncode or r.get('error'):raise RuntimeError((words,r))
        return r
    def launch():
        nonlocal running
        running=True;ctl('launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world',old_world.name,'--creative','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)
    def campaign():
        r=ctl('civilization-state');assert r['enabled'] and not r.get('error'),r
        return r['campaign']
    def check(name):checks.append(name);print('PASS '+name,flush=True)
    def old_view(data):
        result=copy.deepcopy(data);result.pop('version');result.pop('settlements',None);result.pop('next_settlement',None)
        for c in result['civilizations'].values():
            c.pop('settlement_id',None);c.pop('needs',None);c.pop('governance',None);c.pop('economy',None);c.pop('population',None)
            for q in c['quests']:q.pop('delivery',None)
        for c in result.get('choices',{}).values():c.pop('settlement_id',None)
        return result
    def population_view(data):
        result=copy.deepcopy(data)
        for field in ('last_tick','active_ticks','meal_in'):result.pop(field,None)
        return result
    def same_schema_view(data):
        result=copy.deepcopy(data)
        for city in result['civilizations'].values():
            city['population']=population_view(city['population'])
            assessment=city['governance']['assessment']
            if assessment:
                assessment.pop('tick',None);assessment.pop('sample_tick',None)
        return result

    try:
        launch();ctl('wait',35);state=campaign();assert state['version']==8
        if old['version']==8:
            assert same_schema_view(state)==same_schema_view(old),(old,state)
        else:
            assert old_view(state)==old_view(old),(old,state)
        assert len(state['settlements'])==len(old['civilizations'])
        if old['version']<=3:
            assert all(t['kind']=='legacy' and t['status']=='potential-capital' and not t['building_started'] for t in state['settlements'].values())
        else:
            assert state['settlements']==old['settlements'] and state['next_settlement']==old['next_settlement']
        if old['version']==8:
            assert not (world/'horizon-civilization.json.v8.bak').exists()
            check('a real schema-v8 campaign upgrades without migration or a spurious backup, preserving all non-clock state')
        else:
            assert (world/f'horizon-civilization.json.v{old["version"]}.bak').read_bytes()==original
            check('a real old campaign migrates with an exact backup and unchanged membership, leadership, quests and progress')
        expected_stores={}
        expected_populations={}
        for key,city in state['civilizations'].items():
            before=old['civilizations'][key]
            food=sum(q.get('delivery',{}).get('received',{}).get('minecraft:bread',0) for q in before['quests'] if q.get('delivery'))+before.get('governance',{}).get('reserve_bread',0)
            s=city['economy']
            if old['version']>=7:
                assert s==before['economy']
            else:
                assert s['imported_food']==food and s['balance']==({'minecraft:bread':food} if food else {})
                assert not any(s[k] for k in ('deposited','withdrawn','consumed','produced'))
            if old['version']==8:
                assert population_view(city['population'])==population_view(before['population'])
            else:
                assert all(w['job'] is None for w in city['population']['workers'].values())
            expected_stores[key]=copy.deepcopy(s)
            expected_populations[key]=population_view(city['population'])
            if old['version']>=6:
                assert city['governance']['grant']==before['governance']['grant'] and city['governance']['legacy_title']==before['governance']['legacy_title']
        if old['version']==8:
            check('schema-v8 stock, citizen assignments, progress and production/ration counters remain exact')
        else:
            check('version 7 stock is preserved exactly; older food is imported once and citizens receive no automatic jobs')
        for city in state['civilizations'].values():
            b=city['bell'];ctl('tp',b['x']+.5,b['y'],b['z']-2.5);ctl('wait',35)
            assert 'minecraft:bell' in json.dumps(ctl('getblock',b['x'],b['y'],b['z']))
            town=state['settlements'][city['settlement_id']]
            if old['version']<=3:assert town['npc_owner']=='civilization'
            else:assert city['settlement_id']==old['civilizations'][city['id']]['settlement_id']
            assert town['residents']==[q['npc_key'] for q in city['quests']]
        ctl('save')
        actual=json.loads((world/'horizon-owned-npcs.json').read_text(encoding='utf8'))
        economic_keys={(state['settlements'][c['settlement_id']]['npc_owner'],key)
            for c in state['civilizations'].values() if c['stage']=='elder'
            for key in state['settlements'][c['settlement_id']]['residents']}
        previous={(n['owner'],n['stable_key']):n for n in source_npcs['records']}
        def check_npc_metadata(records):
            for n in records['records']:
                identity=(n['owner'],n['stable_key'])
                if old['version']<8 and identity in economic_keys:
                    assert n['profession']=='unassigned' and n['order']['kind']=='idle'
                    old_npc=previous[identity]
                    assert n['order_revision']==old_npc['order_revision']+int(old_npc['profession']!='unassigned')
                    if old_npc['profession']!='unassigned':assert n['order_status']=='cancelled'
        check_npc_metadata(actual)
        # Movement/presence are runtime state; identity and historical orders must survive.
        def identities(value):
            if isinstance(value,list):return [identities(v) for v in value]
            if isinstance(value,dict):
                omitted={'position','presence'}
                if old['version']<8 and (value.get('owner'),value.get('stable_key')) in economic_keys:omitted|={'profession','order_revision','order_status'}
                return {k:identities(v) for k,v in value.items() if k not in omitted}
            return value
        assert identities(actual)==identities(source_npcs)
        check('legacy towns retain their original bells, citizen keys, definitions and saved orders without rebuilding')
        expected=campaign();ctl('quit');running=False;time.sleep(1);launch();ctl('wait',25)
        restored=campaign()
        if old['version']==8:
            assert same_schema_view(restored)==same_schema_view(old)
            assert not (world/'horizon-civilization.json.v8.bak').exists()
        else:
            assert old_view(restored)==old_view(expected)
        assert {key:c['economy'] for key,c in restored['civilizations'].items()}==expected_stores
        assert {key:population_view(c['population']) for key,c in restored['civilizations'].items()}==expected_populations
        ctl('save')
        restored_npcs=json.loads((world/'horizon-owned-npcs.json').read_text(encoding='utf8'))
        check_npc_metadata(restored_npcs)
        assert identities(restored_npcs)==identities(source_npcs)
        assert (old_world/'horizon-civilization.json').read_bytes()==original
        check('a second restart is stable and the original proof world remains unchanged')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        if running:
            try:ctl('quit',timeout=20)
            except Exception as exc:transcript.append({'cleanup':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,'home':str(home),'source':str(source),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
