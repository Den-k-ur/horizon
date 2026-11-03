"""Real two-client settlement orders, partial receipts, restart, rollback and adaptive demand."""
import argparse
import copy
import ctypes
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser=argparse.ArgumentParser(__doc__)
    for name in ('game','ctl','resources','proof-dir'):parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--language',choices=('en_us','ru_ru'),default='en_us')
    args=parser.parse_args();proof=args.proof_dir.resolve();proof.mkdir(parents=True,exist_ok=True)
    homes={who:Path(tempfile.mkdtemp(prefix='civilization-needs-'+who+'-',dir=proof)) for who in ('host','guest')}
    for home in homes.values():(home/'options.txt').write_text('lang:'+args.language+'\n',encoding='utf8')
    sockets={who:home/'control.json' for who,home in homes.items()};running=set();checks=[];transcript=[];success=False;error=None
    guest_name='NeedsGuest'+uuid.uuid4().hex[:6]
    def env(who):
        result=os.environ.copy()
        for key in ('HORIZON_HOME','HORIZON_LAUNCHER_HOME','XDG_DATA_HOME','XDG_CONFIG_HOME','HORIZON_LAUNCHER_TRASH','HORIZON_LAUNCHER_CLIPBOARD','HORIZON_LAUNCHER_DESKTOP'):result[key]=str(homes[who])
        result.update(HORIZON_NO_AUDIO='1',HORIZON_LAUNCHER_NO_SELF_UPDATE='1');return result
    def ctl(who,*words,timeout=30):
        words=list(map(str,words));(proof/'progress.json').write_text(json.dumps({'client':who,'command':words}),encoding='utf8')
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            p=subprocess.run([str(args.ctl.resolve()),'--socket',str(sockets[who]),'--timeout',str(timeout),*words],cwd=homes[who],env=env(who),stdout=out,stderr=err,timeout=timeout+15)
            out.seek(0);err.seek(0);stdout=out.read().decode('utf8',errors='replace');stderr=err.read().decode('utf8',errors='replace')
        if not stdout.strip():raise RuntimeError((who,words,stderr))
        r=json.loads(stdout.strip().splitlines()[-1]);transcript.append({'client':who,'command':words,'reply':r})
        if p.returncode or r.get('error'):raise RuntimeError((who,words,r))
        return r
    def until(predicate,message,timeout=60):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            value=predicate()
            if value:return value
            time.sleep(.2)
        raise AssertionError('Timed out: '+message)
    def check(name):checks.append(name);print('PASS '+name,flush=True)
    def command(who,text):
        r=ctl(who,'command',text);ctl(who,'wait',5);return r
    def campaign():
        r=ctl('host','civilization-state');assert r['enabled'] and not r.get('error'),r
        data=r['campaign']
        for c in data['civilizations'].values():
            a=c.get('governance',{}).get('assessment')
            if a:a.pop('tick',None);a.pop('sample_tick',None)
            population=c.get('population')
            if population:
                for field in ('last_tick','active_ticks','meal_in'):population.pop(field,None)
        return data
    def city():return campaign()['civilizations']['civ-1']
    def quest(id):return next(q for q in city()['quests'] if q['id']==id)
    def counts(who):return ctl(who,'inventory')['counts']
    def launch_host():
        running.add('host');ctl('host','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world','NeedsProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)
    def launch_guest():
        port=ctl('host','lan','open',0)['port'];running.add('guest')
        ctl('guest','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--offscreen','--no-pause','--render-distance',6,timeout=180)
        ctl('guest','connect',f'127.0.0.1:{port}',guest_name);ctl('guest','wait-for','loaded','--timeout',180,timeout=190)
    def resident(q):
        state=campaign();town=state['settlements'][city()['settlement_id']]
        return next(n for n in ctl('host','owned-npcs',json.dumps({'owner':town['npc_owner'],'command':{'operation':'list'}}))['npcs'] if n['stable_key']==q['npc_key'])
    def near(who,q):
        p=resident(q)['position']
        if who=='host':ctl('host','tp',p['x']+2,p['y'],p['z'])
        else:command('host',f'/tp {guest_name} {p["x"]+2} {p["y"]} {p["z"]}')
        ctl(who,'wait',5);ctl(who,'look-at',p['x'],p['y']+1,p['z']);return p
    def give(who,item,n):
        before=counts(who).get(item,0)
        if who=='host':ctl(who,'give',item,n)
        else:command('host',f'/give {guest_name} {item} {n}')
        until(lambda:counts(who).get(item,0)==before+n,'fixture goods')
    def fixture(x,beds):
        ctl('host','tp',x+.5,100,-2.5);ctl('host','wait',40)
        ctl('host','fill',x-12,100,-12,x+12,106,12,'minecraft:air');ctl('host','fill',x-12,99,-12,x+12,99,12,'minecraft:stone')
        ctl('host','setblock',x,100,0,'minecraft:bell[attachment=floor,facing=north,powered=false]')
        for z in range(3,3+beds*2,2):
            ctl('host','setblock',x+3,100,z,'minecraft:red_bed[facing=east,part=foot,occupied=false]')
            ctl('host','setblock',x+4,100,z,'minecraft:red_bed[facing=east,part=head,occupied=false]')
        ctl('host','tp',x+.5,100,-2.5);ctl('host','wait',3)
    def translations(value):
        if isinstance(value,list):return sum((translations(v) for v in value),[])
        if isinstance(value,dict):return ([value['translate']] if 'translate' in value else [])+sum((translations(v) for v in value.values()),[])
        return []
    try:
        launch_host();until(lambda:counts('host').get('minecraft:paper')==1,'starter');founder=campaign()['introduced'][0]
        fixture(0,2);ctl('host','select','minecraft:paper');ctl('host','interact',0,100,0,'north')
        until(lambda:len(campaign()['civilizations'])==1,'foundation')
        until(lambda:city()['governance']['assessment'] is not None,'initial survey')
        until(lambda:ctl('host','owned-npc-mirrors')['count']==3,'three visible own residents')
        assert city()['needs']=={'rules':1,'population':3,'beds':2},city()
        assert quest('quarry')['materials']==[{'item':'minecraft:cobblestone','count':24},{'item':'minecraft:stone_pickaxe','count':1}]
        assert quest('granary')['materials']==[{'item':'minecraft:bread','count':12}]
        assert quest('housing')['materials']==[{'item':'minecraft:oak_planks','count':6},{'item':'minecraft:white_wool','count':6}]
        check('orders are derived from three own residents, two complete beds and earmarked starter reserves')
        near('host',quest('quarry'));ctl('host','use-item')
        until(lambda:'horizon.civilization.quest.quarry.title' in translations(ctl('host','messages',30)['styled']),'real right-click resident dialogue')
        ctl('host','screenshot',proof/'resident-dialogue.png')
        check('right-clicking an independent resident opens a translated server-validated order with delivery action')
        give('host','minecraft:cobblestone',10);command('host','/civ deliver quarry')
        assert quest('quarry')['delivery']['received']=={'minecraft:cobblestone':10}
        assert counts('host').get('minecraft:cobblestone',0)==0 and city()['recognition']==0
        give('host','minecraft:cobblestone',4);before=copy.deepcopy(campaign());inventory=counts('host')
        p=resident(quest('quarry'))['position'];wall=(math.floor(p['x']+1),math.floor(p['y']),math.floor(p['z']))
        for dy in range(3):ctl('host','setblock',wall[0],wall[1]+dy,wall[2],'minecraft:stone')
        command('host','/civ deliver quarry');assert campaign()==before and counts('host')==inventory
        for dy in range(3):ctl('host','setblock',wall[0],wall[1]+dy,wall[2],'minecraft:air')
        check('partial delivery removes exactly ten stone without an early reward; a wall blocks further delivery')
        if os.name=='nt':
            kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.CreateFileW.argtypes=[ctypes.c_wchar_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p];kernel.CreateFileW.restype=ctypes.c_void_p;kernel.CloseHandle.argtypes=[ctypes.c_void_p]
            path=homes['host']/'saves/NeedsProof/horizon-civilization.json';disk=path.read_bytes()
            handle=kernel.CreateFileW(str(path),0x80000000,3,None,3,0,None);assert handle not in (None,ctypes.c_void_p(-1).value)
            try:command('host','/civ deliver quarry');assert campaign()==before and counts('host')==inventory and path.read_bytes()==disk
            finally:kernel.CloseHandle(handle)
            check('real disk replacement failure rolls back partial progress and every removed inventory item')
        launch_guest();until(lambda:len(campaign()['introduced'])==2,'guest starter');guest_id=next(p for p in campaign()['introduced'] if p!=founder)
        before=copy.deepcopy(campaign());command('guest','/civ resident '+quest('quarry')['npc_key']);assert campaign()==before
        command('host','/civ invite '+guest_id);command('guest','/civ join civ-1')
        near('guest',quest('quarry'));give('guest','minecraft:cobblestone',14);command('guest','/civ deliver quarry')
        assert quest('quarry')['delivery']=={'received':{'minecraft:cobblestone':24},'contributors':sorted([founder,guest_id])}
        assert city()['recognition']==0 and quest('quarry')['completed_by'] is None
        expected=copy.deepcopy(campaign());ctl('guest','quit');running.remove('guest');ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1)
        launch_host();launch_guest();until(lambda:campaign()==expected,'partial receipts and two contributors after restart')
        check('non-operator membership is enforced and two contributors retain an incomplete shared order across both client restarts')
        near('guest',quest('quarry'));give('guest','minecraft:stone_pickaxe',1);command('guest','/civ deliver quarry')
        assert city()['recognition']==20 and city()['projects']==1 and quest('quarry')['completed_by']==guest_id
        before=copy.deepcopy(campaign());inventory=counts('guest');command('guest','/civ deliver quarry');assert campaign()==before and counts('guest')==inventory
        check('the last missing tool completes the shared order once with historical finisher and contributors')
        near('host',quest('granary'));give('host','minecraft:bread',20);command('host','/civ deliver granary')
        assert counts('host').get('minecraft:bread')==8 and city()['recognition']==40
        near('host',quest('housing'));give('host','minecraft:oak_planks',6);give('host','minecraft:white_wool',6);command('host','/civ deliver housing')
        for x,block in [(6,'minecraft:crafting_table'),(7,'minecraft:furnace')]:ctl('host','setblock',x,100,1,block)
        ctl('host','setblock',3,100,7,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,7,'minecraft:red_bed[facing=east,part=head,occupied=false]')
        until(lambda:city()['stage']=='elder','stable infrastructure and title',timeout=40)
        assert city()['recognition']==60 and city()['resources']=={'wood':0,'food':0,'housing':3}
        assert quest('granary')['delivery']['received']=={'minecraft:bread':12}
        check('surplus food stays personal, receipts stay earmarked, and only fully supplied orders advance recognition')
        command('guest','/civ leave');assert guest_id in city()['former_members'] and quest('quarry')['completed_by']==guest_id
        fixture(160,4);command('host',f'/tp {guest_name} 160.5 100 -2.5');ctl('guest','wait',60)
        ctl('guest','select','minecraft:paper');ctl('guest','wait',3);ctl('guest','interact',160,100,0,'north')
        until(lambda:len(campaign()['civilizations'])==2,'second foundation')
        until(lambda:campaign()['civilizations']['civ-2']['governance']['assessment'] is not None,'second town survey')
        second=campaign()['civilizations']['civ-2'];assert second['needs']=={'rules':1,'population':3,'beds':4}
        assert [q['id'] for q in second['quests']]==['quarry','granary'] and second['recognition']==0
        before=copy.deepcopy(campaign());command('guest','/civ resident '+quest('quarry')['npc_key']);assert campaign()==before
        owner=campaign()['settlements'][second['settlement_id']]['npc_owner']
        npcs=ctl('host','owned-npcs',json.dumps({'owner':owner,'command':{'operation':'list'}}))['npcs']
        for q,goods in zip(second['quests'],[[('minecraft:cobblestone',24),('minecraft:stone_pickaxe',1)],[('minecraft:bread',12)]]):
            p=next(n['position'] for n in npcs if n['stable_key']==q['npc_key'])
            command('host',f'/tp {guest_name} {p["x"]+2} {p["y"]} {p["z"]}');ctl('guest','wait',5)
            for item,n in goods:give('guest',item,n)
            command('guest','/civ deliver '+q['id'])
        ctl('host','setblock',166,100,1,'minecraft:crafting_table');ctl('host','setblock',167,100,1,'minecraft:furnace')
        until(lambda:campaign()['civilizations']['civ-2']['stage']=='elder','second town live conditions',timeout=40)
        second=campaign()['civilizations']['civ-2']
        assert second['stage']=='elder' and second['recognition']==40 and second['projects']==2 and second['resources']['housing']==4
        check('spare beds remove the housing order; two real orders suffice for the second community without losing former contributions')
        expected=campaign();ctl('guest','quit');running.remove('guest');ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1);launch_host()
        until(lambda:campaign()==expected,'two distinct demand plans after final restart')
        check('completed receipts, contributor history and both distinct plans survive a final server restart')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        for who in ('guest','host'):
            if who in running:
                try:ctl(who,'quit',timeout=20)
                except Exception as exc:transcript.append({'cleanup':who,'error':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,'homes':{k:str(v) for k,v in homes.items()},'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
