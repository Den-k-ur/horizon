"""Real citizens, occupations, nutrition, loaded workplaces and persistence."""
import argparse
import copy
import ctypes
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import time
import uuid


def main():
    parser=argparse.ArgumentParser(__doc__)
    for name in ('game','ctl','resources','proof-dir'):parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--resume-report',type=Path)
    args=parser.parse_args();proof=args.proof_dir.resolve();proof.mkdir(parents=True,exist_ok=True)
    homes={who:Path(tempfile.mkdtemp(prefix='civilization-population-'+who+'-',dir=proof)) for who in ('host','guest')}
    resumed=None
    if args.resume_report:
        resumed=json.loads(args.resume_report.read_text(encoding='utf8'))
        assert not resumed['success'] and len(resumed['checks'])==2
        for who,old in resumed['homes'].items():
            source_home=Path(old)
            if (source_home/'saves').exists():shutil.copytree(source_home/'saves',homes[who]/'saves')
            if (source_home/'horizon-player.txt').exists():shutil.copy2(source_home/'horizon-player.txt',homes[who]/'horizon-player.txt')
    for home in homes.values():(home/'options.txt').write_text('lang:ru_ru\n',encoding='utf8')
    sockets={k:h/'control.json' for k,h in homes.items()};running=set();transcript=[];checks=[];success=False;error=None
    guest_name='GovGuest'+uuid.uuid4().hex[:6]
    if resumed:checks.extend(resumed['checks']);transcript.extend(resumed['transcript'])
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
        return r
    def counts(who):return ctl(who,'inventory')['counts']
    def launch_host():
        running.add('host');ctl('host','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world','PopulationProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',3,'--wait',timeout=180)
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
        if not resumed:
            launch_host();until(lambda:counts('host').get('minecraft:paper')==1,'starter');founder=campaign()['introduced'][0]
            command('host','/gamerule minecraft:spawn_mobs false');command('host','/gamerule minecraft:random_tick_speed 0')
            ctl('host','tp',.5,100,-2.5);ctl('host','wait',20)
            ctl('host','fill',-12,100,-12,12,106,12,'minecraft:air');ctl('host','fill',-12,99,-12,12,99,12,'minecraft:stone')
            ctl('host','setblock',0,100,0,'minecraft:bell[attachment=floor,facing=north,powered=false]')
            for z in (3,5):
                ctl('host','setblock',3,100,z,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,z,'minecraft:red_bed[facing=east,part=head,occupied=false]')
            ctl('host','tp',.5,100,-2.5);ctl('host','wait',5);ctl('host','select','minecraft:paper');ctl('host','interact',0,100,0,'north')
            until(lambda:campaign()['civilizations'] and city()['governance']['assessment'] is not None,'first world census')


        else:
            launch_host();founder=campaign()['civilizations']['civ-1']['founder']
        def pop():return city()['population']
        def stock():return city()['economy']
        def near_bell(who='host'):
            close(who)
            if who=='host':ctl(who,'tp',.5,100,-2.5)
            else:command('host',f'/tp {guest_name} 0.5 100 -2.5')
            ctl(who,'wait',5)
        def worker(key):return pop()['workers'][key]
        def config():return {'revision':pop()['revision'],'jobs':{k:w['job'] for k,w in pop()['workers'].items()}}
        def assign(key,job,x,who='host',rev=None,city_id='civ-1'):
            near_bell(who);rev=pop()['revision'] if rev is None else rev
            command(who,f'/civ workers assign {city_id} {rev} {key} {job} {x} 100 0')
        def release(key,who='host'):
            near_bell(who);command(who,f'/civ workers release civ-1 {pop()["revision"]} {key}');close(who)
        def deposit(item,n):
            near_bell();ctl('host','give',item,n);ctl('host','wait',5)
            command('host',f'/civ stock deposit civ-1 {stock()["revision"]} {item} {n}');close('host')
        def withdraw(item,n):
            near_bell();command('host',f'/civ stock withdraw civ-1 {stock()["revision"]} {item} {n}');close('host')
        if not resumed:
            for id,index,goods in [('quarry',0,[('minecraft:cobblestone',24),('minecraft:stone_pickaxe',1)]),('granary',1,[('minecraft:bread',12)])]:
                near('host',index)
                for item,n in goods:ctl('host','give',item,n)
                ctl('host','wait',5);command('host','/civ deliver '+id)
            for x,block in [(6,'minecraft:crafting_table'),(7,'minecraft:furnace'),(8,'minecraft:crafting_table'),(-8,'minecraft:crafting_table')]:ctl('host','setblock',x,100,0 if abs(x)==8 else 1,block)
            ctl('host','setblock',3,100,7,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,7,'minecraft:red_bed[facing=east,part=head,occupied=false]')
            until(lambda:city()['stage']=='elder' and pop()['observed_population']==3,'live population activated',45)
            assert campaign()['version']==8 and len(pop()['workers'])==3
            keys=sorted(pop()['workers']);baker,carpenter=keys[:2]
            assert all(w['job'] is None for w in pop()['workers'].values())
            deposit('minecraft:wheat',12);deposit('minecraft:oak_log',4)
            before=copy.deepcopy(stock());ctl('host','wait',240);assert stock()==before
            check('the three actual own citizens start unassigned and raw materials alone produce no idle income')
            # A real server sprint advances simulated settlement days, rather than editing campaign data.
            before_rations=pop()['rations'];command('host','/tick sprint 24000')
            until(lambda:pop()['rations']==before_rations+3,'one actual active day feeds three citizens',600)
            command('host','/tick sprint stop');assert stock()['balance']['minecraft:bread']==9
            check('one real active settlement day consumes exactly three bread with a persisted ration account')
            withdraw('minecraft:bread',9);command('host','/tick sprint 24000')
        else:
            assert campaign()['version']==8 and city()['stage']=='elder'
            assert pop()['rations']==3 and pop()['baked']==0 and pop()['carpentered']==0
            assert pop()['meal_in']<=120 and not pop()['hungry']
            assert stock()['balance']=={'minecraft:wheat':12,'minecraft:oak_log':4}
            assert all(w['job'] is None for w in pop()['workers'].values())
            keys=sorted(pop()['workers']);baker,carpenter=keys[:2]
        until(lambda:pop()['hungry'],'a second day without bread causes hunger',600)
        command('host','/tick sprint stop');assert pop()['rations']==3
        assign(baker,'baker',8);close('host');until(lambda:worker(baker)['status']=='hungry','hungry work stops')
        before=copy.deepcopy(stock());ctl('host','wait',220);assert stock()==before
        deposit('minecraft:bread',15);until(lambda:not pop()['hungry'] and pop()['rations']==6,'restocking serves one complete meal')
        release(baker);assert stock()['balance']['minecraft:bread']==12
        check('unfed citizens stop work without generating food; a full real meal restores them without repeating old quest rewards')
        # Default citizen and profession in the actual form; the sole text input is the station.
        near_bell();old_position=next(n['position'] for n in residents() if n['stable_key']==baker)
        panel('host','population_assign');ctl('host','screenshot',proof/'assignment-form-ru.png');ctl('host','type','8 100 0');click('host','Подтвердить');dialog('host');close('host')
        until(lambda:worker(baker)['job'] and worker(baker)['progress']>0,'assigned worker reaches the table',90)
        new_position=next(n['position'] for n in residents() if n['stable_key']==baker)
        assert (new_position['x']-old_position['x'])**2+(new_position['z']-old_position['z'])**2>4
        d=panel('host','population');assert any('Пекарь' in t and 'Работает' in t for t in d['body']),d
        ctl('host','screenshot',proof/'workers-ru.png');close('host')
        until(lambda:pop()['baked']==1,'one baked bread');release(baker)
        assert stock()['balance']['minecraft:wheat']==9 and stock()['balance']['minecraft:bread']==13
        assert stock()['consumed']['minecraft:wheat']==3 and stock()['produced']['minecraft:bread']==1
        check('the real assignment form sends an actual citizen to a player-built table and converts exactly three wheat into bread')
        assign(carpenter,'carpenter',-8);close('host');until(lambda:pop()['carpentered']==1,'carpenter output',90);release(carpenter)
        assert stock()['balance']['minecraft:oak_log']==3 and stock()['balance']['minecraft:oak_planks']==4
        check('assigning a different citizen as carpenter consumes one stored log and produces four real spendable planks')
        before=config()
        for key,job,x,options in [(baker,'baker',80,{}),('foreign','baker',8,{}),(baker,'baker',8,{'city_id':'civ-2'}),(baker,'baker',8,{'rev':0})]:
            assign(key,job,x,**options);close('host');assert config()==before
        assign(baker,'baker',8);close('host');before=config();assign(carpenter,'carpenter',8);close('host');assert config()==before
        # Missing beds and a demolished table preserve input and partial work.
        ctl('host','setblock',3,100,7,'minecraft:air');until(lambda:worker(baker)['status']=='no-housing','missing bed pauses work')
        before=copy.deepcopy(stock());progress=worker(baker)['progress'];ctl('host','wait',220)
        assert stock()==before and worker(baker)['progress']==progress
        ctl('host','setblock',3,100,7,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,7,'minecraft:red_bed[facing=east,part=head,occupied=false]');ctl('host','setblock',8,100,0,'minecraft:air')
        until(lambda:worker(baker)['status']=='no-workplace','destroyed table pauses work');ctl('host','wait',220);assert stock()==before
        ctl('host','setblock',8,100,0,'minecraft:crafting_table');until(lambda:pop()['baked']==2,'repair resumes work');release(baker)
        check('server-side territory, identity, revision and exclusive-station checks reject bad assignments; housing and physical station losses visibly pause production')
        # A cage around the worker proves path reachability is separate from a valid table.
        n=next(n for n in residents() if n['stable_key']==baker);x=int(n['position']['x']//1);z=int(n['position']['z']//1)
        walls={}
        for dx,dz in [(0,-1),(1,0),(0,1),(-1,0)]:
            for y in (100,101,102):
                pos=(x+dx,y,z+dz);walls[pos]=ctl('host','getblock',*pos)['block'];ctl('host','setblock',*pos,'minecraft:stone')
        assign(baker,'baker',-8);close('host');until(lambda:worker(baker)['status']=='blocked','physical route blocked',90)
        before=copy.deepcopy(stock());ctl('host','wait',220);assert stock()==before
        for pos,block in walls.items():ctl('host','setblock',*pos,block)
        until(lambda:pop()['baked']==3,'cleared path resumes worker',90);release(baker)
        check('a real blocked path prevents work and material changes; removing the barrier lets the citizen resume')
        # Preserve an incomplete work cycle across the real host process restart.
        assign(baker,'baker',8);close('host');until(lambda:20<=worker(baker)['progress']<=100,'partial work before restart',90)
        before=copy.deepcopy(stock());previous_progress=worker(baker)['progress'];ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1)
        launch_host();until(lambda:pop()['baked']==4,'saved incomplete cycle completes once',90);release(baker)
        assert stock()['produced']['minecraft:bread']==4 and stock()['consumed']['minecraft:wheat']==12
        assert pop()['rations']==6 and stock()['balance'].get('minecraft:wheat',0)==0
        check('partial work and citizen identity survive a real restart; completing the saved cycle consumes its input once')
        # No material is fabricated while a live assigned worker has no input.
        assign(baker,'baker',8);close('host');until(lambda:worker(baker)['status']=='no-materials','input shortage')
        before=copy.deepcopy(stock());ctl('host','wait',220);assert stock()==before;release(baker)
        check('an assigned, housed and fed worker with no raw materials explains the stop and creates no output')
        # File replacement failure rolls back both config and autonomous material conversion.
        deposit('minecraft:wheat',6);assign(baker,'baker',8);close('host');until(lambda:60<=worker(baker)['progress']<=120,'work before file failure')
        if os.name=='nt':
            kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.CreateFileW.argtypes=[ctypes.c_wchar_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p];kernel.CreateFileW.restype=ctypes.c_void_p;kernel.CloseHandle.argtypes=[ctypes.c_void_p]
            path=homes['host']/'saves/PopulationProof/horizon-civilization.json';h=kernel.CreateFileW(str(path),0x80000000,3,None,3,0,None);assert h not in (None,ctypes.c_void_p(-1).value)
            try:
                disk=path.read_bytes();before=copy.deepcopy(stock());progress=worker(baker)['progress'];configuration=config()
                release(baker);ctl('host','wait',260)
                assert config()==configuration and worker(baker)['progress']==progress and stock()==before and path.read_bytes()==disk
            finally:kernel.CloseHandle(h)
            until(lambda:pop()['baked']==5,'retry after file replacement allowed');release(baker)
            assert stock()['consumed']['minecraft:wheat']==15 and stock()['produced']['minecraft:bread']==5
            check('real Windows save failure rolls back assignment and autonomous progress/output; recovery commits one cycle')
        # A second real client gets read-only administration, then receives authority.
        launch_guest();until(lambda:len(campaign()['introduced'])==2,'guest identity');guest_id=next(p for p in campaign()['introduced'] if p!=founder)
        command('host','/civ invite '+guest_id);command('guest','/civ join civ-1');near_bell('guest')
        d=panel('guest','population');assert 'Назначить работника' not in d['buttons'];close('guest')
        before=config();assign(baker,'baker',8,'guest');close('guest');assert config()==before
        near_bell();command('host','/civ transfer '+guest_id);before=config();assign(baker,'baker',8);close('host');assert config()==before
        assign(baker,'baker',8,'guest');dialog('guest');close('guest');until(lambda:pop()['baked']==6,'non-operator Elder assignment works');release(baker,'guest')
        assert stock()['produced']['minecraft:bread']==6 and stock()['consumed']['minecraft:wheat']==18
        check('a non-operator member can view but cannot assign; transferred leadership enables assignments and immediately revokes the former leader')
        # Freeze a short saved partial cycle by unloading the whole town; no remote income.
        command('host',f'/tp {guest_name} 4000 110 4000');ctl('host','tp',4000,110,4000);ctl('host','wait',400)
        frozen=copy.deepcopy(pop());before=copy.deepcopy(stock());ctl('host','wait',400)
        assert pop()==frozen and stock()==before
        near_bell();near_bell('guest');until(lambda:pop()['last_tick']>frozen['last_tick'],'town reactivated')
        assert stock()==before
        check('an unloaded settlement pauses needs and production without remote catch-up income, and resumes when loaded')
        expected_stock=copy.deepcopy(stock());expected_config=config();expected_rations=pop()['rations'];expected_completed={k:w['completed'] for k,w in pop()['workers'].items()}
        ctl('guest','quit');running.remove('guest');ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1)
        launch_host();launch_guest();until(lambda:city()['leader']==guest_id and pop()['observed_population']==3,'both clients return')
        assert stock()==expected_stock and config()==expected_config and pop()['rations']==expected_rations
        assert {k:w['completed'] for k,w in pop()['workers'].items()}==expected_completed
        d=panel('guest','population');assert 'Назначить работника' in d['buttons'];ctl('guest','screenshot',proof/'population-restored-ru.png');close('guest')
        check('both client restarts preserve exact production and ration accounts, assignments, historical work and current authority')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        for who in ('guest','host'):
            if who in running:
                try:ctl(who,'quit',timeout=20)
                except Exception as exc:transcript.append({'cleanup':who,'error':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'resumed_from':str(args.resume_report) if args.resume_report else None,'prior_error':resumed['error'] if resumed else None,'checks':checks,'homes':{k:str(v) for k,v in homes.items()},'seen_stability':sorted(locals().get('seen',set())),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
