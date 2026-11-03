"""Real warehouse items, processing, allocation, co-op races and rollback."""
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
    homes={who:Path(tempfile.mkdtemp(prefix='civilization-economy-'+who+'-',dir=proof)) for who in ('host','guest')}
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
        running.add('host');ctl('host','launch','--game',args.game.resolve(),'--resources',args.resources.resolve(),'--world','EconomyProof','--seed',42,'--creative','--civilization','--offscreen','--no-pause','--render-distance',6,'--wait',timeout=180)
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

        def stock():return city()['economy']
        def near_bell(who):
            close(who)
            if who=='host':ctl(who,'tp',.5,100,-2.5)
            else:command('host',f'/tp {guest_name} 0.5 100 -2.5')
            ctl(who,'wait',5)
        def stock_cmd(who,op,item,n,revision=None,city_id='civ-1'):
            close(who);revision=stock()['revision'] if revision is None else revision
            return command(who,f'/civ stock {op} {city_id} {revision} {item} {n}')
        def amount(who,n):
            screen=ctl(who,'screen');button=next(b for b in screen['buttons'] if b['label'] in ('Внести предметы','Выдать предметы'))
            ctl(who,'click-at',button['x']+30,button['y']-20)
            ctl(who,'key','right','tap');ctl(who,'key','right','tap');ctl(who,'key','backspace','tap');ctl(who,'key','backspace','tap');ctl(who,'type',n)
        assert campaign()['version']==8 and stock()['balance']=={} and stock()['imported_food']==0
        for id,index,goods in [('quarry',0,[('minecraft:cobblestone',24),('minecraft:stone_pickaxe',1)]),('granary',1,[('minecraft:bread',12)])]:
            near('host',index)
            for item,n in goods:ctl('host','give',item,n)
            ctl('host','wait',5);command('host','/civ deliver '+id)
        for x,block in [(6,'minecraft:crafting_table'),(7,'minecraft:furnace')]:ctl('host','setblock',x,100,1,block)
        ctl('host','setblock',3,100,7,'minecraft:red_bed[facing=east,part=foot,occupied=false]');ctl('host','setblock',4,100,7,'minecraft:red_bed[facing=east,part=head,occupied=false]')
        until(lambda:city()['stage']=='elder','title with actual available bread',45)
        assert stock()['balance']=={'minecraft:bread':12} and stock()['deposited']=={'minecraft:bread':12}
        assert city()['recognition']==40
        check('real order bread enters spendable stock once; construction receipts are not duplicated')
        near_bell('host');ctl('host','give','minecraft:oak_log',20);ctl('host','wait',5)
        d=panel('host','stock_deposit');assert d['title']=='Общий склад';ctl('host','screenshot',proof/'deposit-form-ru.png')
        amount('host','16');click('host','Внести предметы');dialog('host')
        assert stock()['balance']['minecraft:oak_log']==16 and counts('host').get('minecraft:oak_log')==4
        ctl('host','screenshot',proof/'warehouse-ru.png');close('host')
        check('the Russian deposit form removes exactly the requested real ordinary items and retains surplus')
        before_stock=copy.deepcopy(stock());before_inv=counts('host')
        for op,item,n,extra in [('deposit','minecraft:oak_log',0,{}),('deposit','minecraft:oak_log',65,{}),('deposit','minecraft:diamond',1,{}),('deposit','minecraft:oak_log',5,{}),('withdraw','minecraft:stone',1,{}),('deposit','minecraft:oak_log',1,{'city_id':'civ-2'})]:
            stock_cmd('host',op,item,n,**extra)
            assert stock()==before_stock and counts('host')==before_inv,(op,item,n)
        check('invalid items and amounts, insufficient inventory or stock, and a foreign city leave both stores unchanged')
        # One accepted revision can be used only once, including an immediately repeated packet.
        rev=stock()['revision'];stock_cmd('host','deposit','minecraft:oak_log',2,rev);dialog('host')
        expected=copy.deepcopy(stock());expected_inv=counts('host')
        stock_cmd('host','deposit','minecraft:oak_log',2,rev)
        assert stock()==expected and counts('host')==expected_inv
        check('replaying the same deposit form cannot remove or credit items twice')
        panel('host','stock');click('host','1 дубовое бревно → 4 доски');dialog('host')
        assert stock()['balance']['minecraft:oak_log']==17 and stock()['balance']['minecraft:oak_planks']==4
        assert stock()['consumed']=={'minecraft:oak_log':1} and stock()['produced']=={'minecraft:oak_planks':4}
        close('host');before_inv=counts('host').get('minecraft:oak_log',0)
        panel('host','stock_withdraw');amount('host','3');click('host','Выдать предметы');dialog('host')
        assert stock()['balance']['minecraft:oak_log']==14 and counts('host').get('minecraft:oak_log',0)==before_inv+3
        close('host');stock_cmd('host','withdraw','minecraft:oak_planks',4);dialog('host');close('host')
        # Allocate to a real manual building placement, rather than a counter-only expense.
        ctl('host','tp',.5,100,-2.5);command('host','/gamemode survival');ctl('host','select','minecraft:oak_planks');ctl('host','wait',5);ctl('host','interact',2,99,-2,'up');ctl('host','wait',5)
        assert counts('host').get('minecraft:oak_planks',0)==3
        assert ctl('host','getblock',2,100,-2)['block']=='minecraft:oak_planks'
        command('host','/gamemode creative')
        check('real processing spends a log, produces four planks, and allocated planks build an actual world block')
        # Outflow changes current needs, never historical order receipts or the title.
        history=copy.deepcopy(city()['quests']);near_bell('host');stock_cmd('host','withdraw','minecraft:bread',4);dialog('host');close('host')
        assert stock()['balance']['minecraft:bread']==8 and city()['governance']['assessment']['stable_samples']==0
        assert city()['stage']=='elder' and city()['quests']==history
        d=panel('host');assert any('Хлеб: 8' in t for t in d['body']),d;close('host')
        near('host',1);command('host','/civ provision');assert stock()['balance']['minecraft:bread']==12 and city()['quests']==history
        check('food allocation lowers live wellbeing and stability; restocking preserves receipts and cannot re-award recognition')
        near_bell('host');before_stock=copy.deepcopy(stock());before_inv=counts('host')
        ctl('host','tp',80,100,0);stock_cmd('host','deposit','minecraft:oak_log',1)
        assert stock()==before_stock and counts('host')==before_inv
        near_bell('host');ctl('host','setblock',0,100,-1,'minecraft:stone');ctl('host','setblock',0,101,-1,'minecraft:stone')
        stock_cmd('host','deposit','minecraft:oak_log',1);assert stock()==before_stock and counts('host')==before_inv
        ctl('host','setblock',0,100,-1,'minecraft:air');ctl('host','setblock',0,101,-1,'minecraft:air')
        command('host','/gamemode spectator');stock_cmd('host','withdraw','minecraft:oak_log',1)
        assert stock()==before_stock and counts('host')==before_inv;command('host','/gamemode creative')
        check('remote, obstructed and spectator requests cannot move warehouse items')
        # A full inventory is a failed allocation, without drops or partial stock loss.
        command('host','/clear')
        for group,n in [('hotbar',9),('inventory',27)]:
            for i in range(n):command('host',f'/item replace entity @s {group}.{i} with minecraft:dirt 64')
        command('host','/item replace entity @s weapon.offhand with minecraft:dirt 64')
        before_stock=copy.deepcopy(stock());before_inv=counts('host');stock_cmd('host','withdraw','minecraft:oak_log',1)
        assert stock()==before_stock and counts('host')==before_inv
        command('host','/clear');ctl('host','give','minecraft:oak_log',4)
        check('a full inventory rolls back allocation completely without world drops')
        if os.name=='nt':
            kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.CreateFileW.argtypes=[ctypes.c_wchar_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p];kernel.CreateFileW.restype=ctypes.c_void_p;kernel.CloseHandle.argtypes=[ctypes.c_void_p]
            path=homes['host']/'saves/EconomyProof/horizon-civilization.json';h=kernel.CreateFileW(str(path),0x80000000,3,None,3,0,None)
            assert h not in (None,ctypes.c_void_p(-1).value)
            try:
                disk=path.read_bytes();before_stock=copy.deepcopy(stock());before_inv=counts('host')
                stock_cmd('host','deposit','minecraft:oak_log',2)
                assert stock()==before_stock and counts('host')==before_inv and path.read_bytes()==disk
                stock_cmd('host','withdraw','minecraft:oak_log',2)
                assert stock()==before_stock and counts('host')==before_inv and path.read_bytes()==disk
            finally:kernel.CloseHandle(h)
            check('real Windows file replacement failure rolls back deposits and allocations in memory and inventory')
        launch_guest();until(lambda:len(campaign()['introduced'])==2,'guest identity');guest_id=next(p for p in campaign()['introduced'] if p!=founder)
        command('host','/civ invite '+guest_id);command('guest','/civ join civ-1');near_bell('guest')
        command('host',f'/give {guest_name} minecraft:iron_ingot 5');ctl('guest','wait',5)
        before=counts('guest').get('minecraft:iron_ingot',0)
        stock_cmd('guest','deposit','minecraft:iron_ingot',3);dialog('guest')
        assert stock()['balance']['minecraft:iron_ingot']==3 and counts('guest').get('minecraft:iron_ingot',0)==before-3
        d=dialog('guest');assert 'Выдать предметы' not in d['buttons'] and '1 дубовое бревно → 4 доски' not in d['buttons'];close('guest')
        before_stock=copy.deepcopy(stock());stock_cmd('guest','withdraw','minecraft:iron_ingot',1);assert stock()==before_stock
        check('a non-operator member contributes industrial items but cannot allocate them or process resources')
        # Both clients submit the same stock version concurrently; only one deposit wins.
        near_bell('host');near_bell('guest');command('host',f'/give {guest_name} minecraft:oak_log 4');ctl('guest','wait',5)
        rev=stock()['revision'];before_stock=copy.deepcopy(stock());before_h=counts('host').get('minecraft:oak_log',0);before_g=counts('guest').get('minecraft:oak_log',0)
        import concurrent.futures
        def compete(who):
            reply=subprocess.run([str(args.ctl.resolve()),'--socket',str(sockets[who]),'--timeout','20','command',f'/civ stock deposit civ-1 {rev} minecraft:oak_log 2'],capture_output=True,encoding='utf8',timeout=30)
            assert reply.returncode==0,reply.stdout
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:list(pool.map(compete,['host','guest']))
        ctl('host','wait',10);assert stock()['revision']==rev+1
        assert stock()['balance']['minecraft:oak_log']==before_stock['balance']['minecraft:oak_log']+2
        assert counts('host').get('minecraft:oak_log',0)+counts('guest').get('minecraft:oak_log',0)==before_h+before_g-2
        close('host');close('guest')
        check('simultaneous host and non-operator guest deposits with one revision accept exactly one inventory transfer')
        # Transfer does not grant stale authority to the former leader's open form.
        near_bell('host');rev=stock()['revision'];command('host','/civ transfer '+guest_id)
        before_stock=copy.deepcopy(stock());stock_cmd('host','withdraw','minecraft:oak_log',1,rev);assert stock()==before_stock
        stock_cmd('guest','withdraw','minecraft:iron_ingot',2);dialog('guest');close('guest')
        assert stock()['balance']['minecraft:iron_ingot']==1
        d=panel('guest','stock_history');assert any('выдано 2' in t for t in d['body']),d
        ctl('guest','screenshot',proof/'resource-accounts-ru.png');close('guest')
        check('leadership transfer revokes allocation immediately and the new Elder sees the persisted movement accounts')
        expected=copy.deepcopy(stock());expected_h=counts('host');expected_g=counts('guest')
        ctl('guest','quit');running.remove('guest');ctl('host','save');ctl('host','quit');running.remove('host');time.sleep(1)
        launch_host();launch_guest();until(lambda:stock()==expected,'stock restored without duplicate import')
        assert counts('host')==expected_h and counts('guest')==expected_g
        assert city()['leader']==guest_id and city()['quests']==history
        check('both client restarts preserve exact balances, accounting, inventories, contribution history and authority')
        success=True
    except BaseException as exc:error=repr(exc);raise
    finally:
        for who in ('guest','host'):
            if who in running:
                try:ctl(who,'quit',timeout=20)
                except Exception as exc:transcript.append({'cleanup':who,'error':repr(exc)})
        (proof/'report.json').write_text(json.dumps({'success':success,'error':error,'checks':checks,'homes':{k:str(v) for k,v in homes.items()},'seen_stability':sorted(locals().get('seen',set())),'transcript':transcript},ensure_ascii=False,indent=2),encoding='utf8')


if __name__=='__main__':main()
