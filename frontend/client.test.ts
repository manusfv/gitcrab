import { describe, expect, it, vi } from 'vitest';
import { Client, LatestLane } from './client';
const invoke = vi.hoisted(()=>vi.fn());
vi.mock('@tauri-apps/api/core',()=>({invoke}));
function deferred<T>(){let resolve!:(value:T)=>void;const promise=new Promise<T>(r=>resolve=r);return {promise,resolve}}
describe('application request reconciliation',()=>{
  it('drops replies from a previous repository including failures',async()=>{
    const client=new Client(),pending=deferred<unknown>();invoke.mockReturnValueOnce(pending.promise);
    const reply=client.request({kind:'Refresh',args:{limit:500}},'snapshot');client.epoch++;
    pending.resolve({epoch:0,sequence:1,result:{Err:{message:'old failure',refresh:true}}});expect(await reply).toBeUndefined();
  });
  it('keeps the latest selected file when an older diff completes late',async()=>{
    const client=new Client(),first=deferred<unknown>();invoke.mockReturnValueOnce(first.promise);
    const old=client.request({kind:'Diff',args:{id:null,path:'a',staged:false}},'diff');
    invoke.mockResolvedValueOnce({epoch:0,sequence:2,result:{Ok:{kind:'Done',data:{message:'latest',committed_draft:null}}}});
    const latest=await client.request({kind:'Diff',args:{id:null,path:'b',staged:false}},'diff');
    first.resolve({epoch:0,sequence:1,result:{Ok:{kind:'Done',data:{message:'obsolete',committed_draft:null}}}});
    expect(await old).toBeUndefined();expect(latest?.kind).toBe('Done');
  });
  it('coalesces rapid read selections to one current and one latest task',async()=>{
    const lane=new LatestLane(),pending=deferred<void>(),visited:string[]=[];
    lane.run(async()=>{visited.push('running');await pending.promise});
    lane.run(async()=>{visited.push('obsolete')});lane.run(async()=>{visited.push('latest')});
    pending.resolve();await pending.promise;await Promise.resolve();
    expect(visited).toEqual(['running','latest']);
  });
});
