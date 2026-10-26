// Execute typed helper calls and ARM carry laws emitted by Rust.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {withSource,type Fn} from './source.test.mts';
const filename=process.argv[2];assert(filename);
const memory=new WebAssembly.Memory({initial:2}),table=new WebAssembly.Table({initial:16,element:'anyfunc'}),v=new DataView(memory.buffer);
const env=0x1000,regs=0x2000,uc=0x3000,cpu=0x3200,args=0x4000,pc=0x4100,marker=0x4200;
const module=new WebAssembly.Module(readFileSync(filename));
const instance=new WebAssembly.Instance(module,{env:{memory,table,load:()=>0n,store:()=>{},helper:()=>{throw Error('unexpected generic helper bridge');}}});
const f=withSource(module,instance,memory);
// Independent minimal helper fixtures use the native (i32)->i64 signature.
const leb=(n:number)=>{const out:number[]=[];do{const b=n&127;n>>>=7;out.push(b|(n?128:0));}while(n);return out;};
const sint=(n:number)=>{const out:number[]=[];let more:boolean;do{const b=n&127;n>>=7;more=!((n===0&&!(b&64))||(n===-1&&(b&64)));out.push(b|(more?128:0));}while(more);return out;};
const txt=(s:string)=>[s.length,...new TextEncoder().encode(s)],sec=(n:number,b:number[])=>[n,...leb(b.length),...b],i32=(n:number)=>[0x41,...sint(n)];
function fixture(code:number[]){
    const body=[0,...code,0x0b],bytes=[0,97,115,109,1,0,0,0,...sec(1,[1,0x60,1,0x7f,1,0x7e]),
        ...sec(2,[1,...txt('env'),...txt('memory'),2,0,1]),...sec(3,[1,0]),...sec(7,[1,...txt('fn'),0,0]),...sec(10,[1,...leb(body.length),...body])];
    return new WebAssembly.Instance(new WebAssembly.Module(new Uint8Array(bytes)),{env:{memory}}).exports.fn as (scratch:number)=>bigint;
}
const mark=[...i32(marker),...i32(1),0x36,2,0];
table.set(1,fixture([...mark,0x20,0,0x29,3,0,0x20,0,0x29,3,8,0x7c,0x20,0,0x29,3,16,0x7c]));
table.set(2,fixture([...mark,...i32(uc),...i32(1),0x3a,0,0,0x42,0]));
table.set(3,fixture([...mark,0x42,0]));
// Native trace signature: (size:i32,index:i32,handle:i32,address:i64)->void.
const traceAt=0x4300,traceControl=0x4320;
{
    const code:number[]=[];
    for(let n=0;n<3;n++)code.push(...i32(traceAt+n*4),0x20,n,0x36,2,0);
    code.push(...i32(traceAt+16),0x20,3,0x37,3,0,...i32(uc),...i32(traceControl),0x2d,0,0,0x3a,0,0);
    const body=[0,...code,0x0b],bytes=[0,97,115,109,1,0,0,0,...sec(1,[1,0x60,4,0x7f,0x7f,0x7f,0x7e,0]),
        ...sec(2,[1,...txt('env'),...txt('memory'),2,0,1]),...sec(3,[1,0]),...sec(7,[1,...txt('fn'),0,0]),...sec(10,[1,...leb(body.length),...body])];
    table.set(4,new WebAssembly.Instance(new WebAssembly.Module(new Uint8Array(bytes)),{env:{memory}}).exports.fn as Function);
}
function reset(){
    v.setUint32(env+128,uc,true);v.setUint8(uc,0);v.setUint32(uc+4,0,true);v.setUint32(uc+8,cpu,true);v.setUint32(cpu,0,true);v.setUint32(regs+56,env,true);v.setUint32(marker,0,true);
}
function invoke(fn:Fn,values:number[],codeBase=0x5000){reset();values.forEach((value,n)=>v.setUint32(regs+n*4,value,true));return fn(env,regs,args,codeBase);}
const wide=0x8877665544332211n;
assert.equal(invoke(f.b0,[0x1234,0x44332211,0x88776655,0xfedcba98]),0x123400000000n);
assert.equal(v.getBigUint64(env,true),wide+0x1234n+0xfedcba98n);
assert.equal(v.getBigUint64(args,true),0x1234n);assert.equal(v.getBigUint64(args+8,true),wide);assert.equal(v.getBigUint64(args+16,true),0xfedcba98n);
assert.equal(v.getUint32(pc,true),0x5002);assert.equal(v.getUint32(marker,true),1);
assert.equal(invoke(f.b0,[0,0,0,0],0x7000),0x123400000000n);assert.equal(v.getUint32(pc,true),0x7002);
assert.equal(invoke(f.b1,[]),0n);assert.equal(v.getUint8(uc),0);assert.equal(v.getUint32(marker,true),1);
for(const [retry,request,delay,slow] of [[0,0,0,false],[0,0,1,false],[3,0,0,true],[3,0,1,true],[0,0x80000000,0,true],[0,0x80000000,1,true]] as const){
    reset();v.setUint32(regs,uc,true);v.setUint32(regs+4,delay,true);v.setUint32(uc+4,retry,true);v.setUint32(cpu,request,true);
    assert.equal(f.b2(env,regs,args,0x5000),0x123400000000n);assert.equal(v.getUint32(marker,true),Number(slow));assert.equal(v.getBigUint64(env,true),0n);
}
for(const base of [0x5000,0x7000])for(const pending of [false,true]){
    reset();new Uint8Array(memory.buffer,args,32).fill(0x6c);v.setUint8(traceControl,Number(pending));
    for(const [r,value] of [[0,0x40000010],[1,3],[2,uc],[3,0x44332211],[5,0x88776655]])v.setUint32(regs+r*4,value,true);
    v.setBigUint64(env,0xfaceb00cn,true);
    assert.equal(f.b7(env,regs,args,base),pending?0n:0x123400000000n);
    assert.deepEqual([0,4,8].map(offset=>v.getUint32(traceAt+offset,true)),[0x40000010,3,uc]);
    assert.equal(v.getBigUint64(traceAt+16,true),wide);assert.equal(v.getUint32(pc,true),base+2);
    assert.equal(v.getUint8(uc),0);assert.equal(v.getBigUint64(env,true),pending?0xfaceb00cn:0n);
    assert(new Uint8Array(memory.buffer,args,32).every(x=>x===0x6c),'direct trace must not use the packing buffer');
}
let cases=0;
for(const [index,kind] of [[3,'shl'],[4,'shr'],[5,'sar'],[6,'ror']] as const){
    const generated=f[`b${index}`];
    for(const x of [0,1,0xffffffff,0x80000000,0x7fffffff,0xa59c3271])for(const carry of [0,1])for(let shift=0;shift<256;shift++){
        let result=x,cf=carry;
        if(shift){
            if(kind==='shl'){cf=shift<=32?(x>>>(32-shift))&1:0;result=shift<32?(x<<shift)>>>0:0;}
            if(kind==='shr'){cf=shift<=32?(x>>>(shift-1))&1:0;result=shift<32?x>>>shift:0;}
            if(kind==='sar'){cf=(x>>>(Math.min(shift,32)-1))&1;result=((x|0)>>Math.min(shift,31))>>>0;}
            if(kind==='ror'){const n=shift&31;result=((x>>>n)|(x<<-n))>>>0;cf=result>>>31;}
        }
        reset();v.setUint32(env+132,carry,true);[env,x,shift+0x100].forEach((value,n)=>v.setUint32(regs+n*4,value,true));
        assert.equal(generated(env,regs,args,0x5000),0x123400000000n);assert.equal(v.getUint32(env,true),result);assert.equal(v.getUint32(env+4,true),0);assert.equal(v.getUint32(env+132,true),cf);cases++;
    }
}
console.log(JSON.stringify({passed:true,carryCases:cases,typedArguments:true,precisePc:true,normalExit:true,exitChecks:true}));
