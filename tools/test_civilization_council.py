"""Real council UI, observed recognition, supply, authority and title persistence."""
import argparse
import copy
import ctypes
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid


def main():
    parser=argparse.ArgumentParser(__doc__)
    for name in ('game','ctl','resources','proof-dir'):parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args();proof=args.proof_dir.resolve();proof.mkdir(parents=True,exist_ok=True)
    homes={who:Path(tempfile.mkdtemp(prefix='civilization-council-'+who+'-',dir=proof)) for who in ('host','guest')}
    for home in homes.values():(home/'options.txt').write_text('lang:ru_ru\n',encoding='utf8')
    sockets={k:h/'control.json' for k,h in homes.items()};running=set();transcript=[];checks=[];success=False;error=None
    guest_name='GovGuest'+uuid.uuid4().hex[:6]
    def env(who):
        e=os.environ.copy()
        for key in ('HORIZON_HOME','HORIZON_LAUNCHER_HOME','XDG_DATA_HOME','XDG_CONFIG_HOME','HORIZON_LAUNCHER_TRASH','HORIZON_LAUNCHER_CLIPBOARD','HORIZON_LAUNCHER_DESKTOP'):e[key]=str(homes[who])
        e.update(HORIZON_NO_AUDIO='1',HORIZON_LAUNCHER_NO_SELF_UPDATE='1');return e
    def ctl(who,*words,timeout=30):
        words=list(map(str,words));(proof/'progress.json').write_text(json.dumps({'client':who,'command':words}),encoding='utf8')
        with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
            p=subprocess.run([str(args.ctl.resolve()),'--socket',str(sockets[who]),'--timeout',str(timeout),*words],env=env(who),cwd=homes[who],stdout=out,stderr=err,timeout=timeout+15)
            out.seek(0);err.seek(0);stdout=out.read().decode('utf8',errors='replace');stderr=err.read().decode('utf8',errors='replace')
        if not stdout.strip():raise RuntimeError((who,words,stderr))
        r=json.loads(stdout.strip().splitlines()[-1]);transcript.append({'client':who,'command':words,'reply':r})
        if p.returncode or r.get('error'):raise RuntimeError((who,words,r))
        return r
    def until(predicate,message,timeout=60):
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            r=predicate()
            if r:return r
            time.sleep(.15)
        raise AssertionError('Timed out: '+message)
    def check(name):checks.append(name);print('PASS '+name,flush=True)
    def command(who,text):
        r=ctl(who,'command',text);ctl(who,'wait',5);return r
    def campaign():
        r=ctl('host','civilization-state');assert r['enabled'] and not r.get('error'),r
        return r['campaign']
    def city():return campaign()['civilizations']['civ-1']
    def comparable():
        r=copy.deepcopy(campaign())
        for c in r['civilizations'].values():
            a=c['governance']['assessment']
            if a:a.pop('tick');a.pop('sample_tick')
            population=c.get('population')
            if population:
                for field in ('last_tick','active_ticks','meal_in'):population.pop(field,None)
        return r
    def counts(who):return ctl(who,'inventory')['counts']
    def launch_host():
        running.add('host');ctl('host','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world','CouncilProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)
    def launch_guest():
        port=ctl('host','lan','open',0)['port'];running.add('guest')
        ctl('guest','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--offscreen','--no-pause','--render-distance',6,timeout=180)
        ctl('guest','connect',f'127.0.0.1:{port}',guest_name);ctl('guest','wait-for','loaded','--timeout',180,timeout=190)
    def close(who):
        if ctl(who,'screen')['screen'] in ('dialog','dialog-warning'):ctl(who,'key','esc','tap')
        until(lambda:ctl(who,'screen')['screen']=='playing','close panel')
    def dialog(who):
        until(lambda:ctl(who,'screen')['screen']=='dialog','panel opened')
        return ctl(who,'state')['dialog']['open']
    def panel(who,page='overview'):
        close(who);command(who,'/civ panel '+page);return dialog(who)
    def click(who,label):ctl(who,'click-button',label);time.sleep(.2)
    def residents():
        c=city();town=campaign()['settlements'][c['settlement_id']]
        return ctl('host','owned-npcs',json.dumps({'owner':town['npc_owner'],'command':{'operation':'list'}}))['npcs']
    def near(who,index):
        c=city();key=campaign()['settlements'][c['settlement_id']]['residents'][index]
        p=next(n['position'] for n in residents() if n['stable_key']==key)
        close(who)
        if who=='host':ctl(who,'tp',p['x']+2,p['y'],p['z'])
        else:command('host',f'/tp {guest_name} {p["x"]+2} {p["y"]} {p["z"]}')
        ctl(who,'wait',5);return p
    try:
        launch_host();until(lambda:counts('host').get('minecraft:paper')==1,'starter');founder=campaign()['introduced'][0]
        ctl('host','tp',.5,100,-2.5);ctl('host','wait',20)
        ctl('host','fill',-12,100,-12,12,106,12,'minecraft:air');ctl('host','fill',-12,99,-12,12,99,12,'minecraft:stone')
        ctl('host','setblock',0,100,0,'minecraft:bell[attachment=floor,facing=north,powered=false]')
        for z in (3,5):
            ctl('host','setblock',3,100,z,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,z,'minecraft:red_bed[facing=east,part=head,occupied=false]')
        ctl('host','tp',.5,100,-2.5);ctl('host','wait',5);ctl('host','select','minecraft:paper');ctl('host','interact',0,100,0,'north')
        until(lambda:campaign()['civilizations'] and city()['governance']['assessment'] is not None,'first world census')
        d=panel('host');assert d['title']=='Признание общины' and any('кроватей 2' in t for t in d['body']),d
        assert 'Пригласить' not in d['buttons'];tick=city()['governance']['assessment']['tick'];ctl('host','wait',110);assert city()['governance']['assessment']['tick']>tick
        ctl('host','screenshot',proof/'recognition-locked-ru.png');close('host')
        before=comparable();command('host','/civ govern invite 00000000000000000000000000000001');assert comparable()==before
        check('a real non-pausing Russian progress panel shows live beds and rejects administration before the title')
        near('host',1);ctl('host','give','minecraft:bread',4);ctl('host','wait',5)
        panel('host');click('host','Передать хлеб');until(lambda:ctl('host','screen')['screen']=='playing','supply action closes panel')
        assert city()['economy']['balance'].get('minecraft:bread',0)==4 and counts('host').get('minecraft:bread',0)==0 and city()['recognition']==0
        ctl('host','give','minecraft:bread',20);ctl('host','wait',5);command('host','/civ provision')
        assert city()['economy']['balance'].get('minecraft:bread',0)==12 and counts('host').get('minecraft:bread')==12 and city()['recognition']==0
        check('the supply action persists partial real bread, retains surplus and cannot farm recognition')
        for id,index,goods in [('quarry',0,[('minecraft:cobblestone',24),('minecraft:stone_pickaxe',1)]),('housing',2,[('minecraft:oak_planks',6),('minecraft:white_wool',6)])]:
            near('host',index)
            for item,n in goods:ctl('host','give',item,n)
            ctl('host','wait',5);command('host','/civ deliver '+id)
        ctl('host','wait',110)
        assert city()['recognition']==40 and city()['stage']=='recognition' and city()['governance']['grant'] is None
        d=panel('host','metrics');assert any('Кровать для каждого' in t and 'нужно выполнить' in t for t in d['body']),d
        ctl('host','screenshot',proof/'missing-infrastructure-ru.png');close('host')
        check('forty recognition and real food do not bypass missing housing and workstations')
        for x,block in [(6,'minecraft:crafting_table'),(7,'minecraft:furnace')]:ctl('host','setblock',x,100,1,block)
        ctl('host','setblock',3,100,7,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,7,'minecraft:red_bed[facing=east,part=head,occupied=false]')
        seen=set()
        def title_ready():
            c=city();seen.add(c['governance']['assessment']['stable_samples']);return c['stage']=='elder'
        until(title_ready,'three stable world checks',timeout=45)
        c=city();assert c['recognition']==40 and c['projects']==2 and c['quests'][1]['completed_by'] is None
        assert c['resources']['housing']==3 and c['governance']['grant']['observation']['stable_samples']==3
        assert 1 in seen and 3 in seen,seen
        d=panel('host');assert d['title']=='Управление поселением' and any('Староста' in t for t in d['body']),d
        ctl('host','screenshot',proof/'council-overview-ru.png');close('host')
        check('actual third bed, crafting table and furnace unlock Elder after stable checks without forcing a particular order')
        d=panel('host','materials');assert any('Хлеб 12' in t and 'булыжник 24' in t for t in d['body']),d
        ctl('host','screenshot',proof/'council-supplies-ru.png');close('host')
        near('host',1);before=counts('host');command('host','/civ provision');assert counts('host')==before and city()['economy']['balance'].get('minecraft:bread',0)==12
        check('the supplies panel reports real earmarked deliveries and a full reserve cannot consume more bread')
        launch_guest();until(lambda:len(campaign()['introduced'])==2,'guest starter');guest_id=next(p for p in campaign()['introduced'] if p!=founder)
        before=comparable();command('guest','/civ panel');command('guest','/civ govern kick '+founder);assert comparable()==before and ctl('guest','screen')['screen']=='playing'
        panel('host','members');click('host','Пригласить');dialog('host');ctl('host','type',guest_id);click('host','Подтвердить');dialog('host')
        until(lambda:guest_id in city()['invitations'],'UI invitation');close('host');command('guest','/civ join civ-1')
        d=panel('guest','members');assert 'Пригласить' not in d['buttons'] and any('Участник общины' in t for t in d['body']),d
        before=comparable();close('guest');command('guest','/civ govern transfer '+guest_id);assert comparable()==before
        check('the invite form works and an ordinary non-operator member gets a read-only panel with server-side authority checks')
        panel('host','transfer');click('host','Подтвердить');dialog('host');until(lambda:city()['leader']==guest_id,'UI leadership transfer')
        d=dialog('host');assert 'Пригласить' not in d['buttons'];close('host')
        d=panel('guest','members');assert 'Передать руководство' in d['buttons'];ctl('guest','screenshot',proof/'new-elder-members-ru.png')
        close('guest');before=comparable();command('host','/civ govern kick '+guest_id);assert comparable()==before
        panel('guest','kick');click('guest','Подтвердить');dialog('guest');until(lambda:founder not in city()['members'],'UI removal')
        assert founder in city()['former_members'] and city()['quests'][0]['completed_by']==founder
        close('guest');command('host','/civ panel');assert ctl('host','screen')['screen']=='playing'
        check('transfer and removal through actual forms revoke former authority while retaining founder and contributor history')
        ctl('host','setblock',7,100,1,'minecraft:air');ctl('host','wait',110)
        assert city()['stage']=='elder' and city()['governance']['assessment']['stable_samples']==0
        d=panel('guest','metrics');assert any('Общая печь' in t and 'нужно выполнить' in t for t in d['body']),d
        ctl('guest','screenshot',proof/'crisis-title-kept-ru.png');close('guest')
        check('a real infrastructure loss lowers stability and displays the crisis without erasing the earned title')
        if os.name=='nt':
            panel('guest','invite');ctl('guest','type','00000000000000000000000000000001')
            kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.CreateFileW.argtypes=[ctypes.c_wchar_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p];kernel.CreateFileW.restype=ctypes.c_void_p;kernel.CloseHandle.argtypes=[ctypes.c_void_p]
            path=homes['host']/'saves/CouncilProof/horizon-civilization.json'
            h=kernel.CreateFileW(str(path),0x80000000,3,None,3,0,None);assert h not in (None,ctypes.c_void_p(-1).value)
            try:
                disk=path.read_bytes();before=comparable()
                click('guest','Подтвердить');assert comparable()==before and path.read_bytes()==disk
            finally:kernel.CloseHandle(h)
            close('guest');check('a real file replacement failure rolls back a submitted council form without ghost invitations')
        expected=comparable();ctl('guest','quit');running.remove('guest');ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1)
        launch_host();launch_guest();until(lambda:comparable()==expected,'saved title, authority and crisis')
        d=panel('guest');assert d['title']=='Управление поселением' and any('Староста' in t for t in d['body']);close('guest')
        check('both client restarts preserve the title, transferred authority, historical work and actual crisis state')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        for who in ('guest','host'):
            if who in running:
                try:ctl(who,'quit',timeout=20)
                except Exception as exc:transcript.append({'cleanup':who,'error':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,'homes':{k:str(v) for k,v in homes.items()},'seen_stability':sorted(locals().get('seen',set())),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
