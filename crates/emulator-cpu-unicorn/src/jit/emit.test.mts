// Execute Rust-generated modules against independent arithmetic/ABI expectations.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {withSource,type Fn} from './source.test.mts';
const [filename,name]=process.argv.slice(2);
assert(filename&&name,'module file and test case are required');
const memory=new WebAssembly.Memory({initial:4}),table=new WebAssembly.Table({initial:16,element:'anyfunc'});
const view=new DataView(memory.buffer),env=0x1000,bank=0x2000,scratch=0x4000;
let load=(_env:number,_address:number,_oi:number,_pc:number):bigint=>{throw Error('unexpected load');};
let store=(_env:number,_address:number,_value:bigint,_oi:number,_pc:number):void=>{throw Error('unexpected store');};
let helper=(_env:number,_fn:number,_bank:number,_scratch:number,_words:number,_mask:number,_pc:number):number=>{throw Error('unexpected helper');};
const module=new WebAssembly.Module(readFileSync(filename));
const instance=new WebAssembly.Instance(module,{env:{memory,table,
    load:(...args:Parameters<typeof load>)=>load(...args),store:(...args:Parameters<typeof store>)=>store(...args),helper:(...args:Parameters<typeof helper>)=>helper(...args)}});
const raw=instance.exports as Record<string,Fn>,f=withSource(module,instance,memory);
const reg=(r:number,n:number)=>view.setUint32(bank+r*4,n,true);
const invoke=(n:number)=>{reg(14,env);assert.equal(f[`b${n}`](env,bank,scratch,0x5000),0x123400000000n);return view.getUint32(env,true);};
let seed=0x75838abc;
const random=()=>{seed^=seed<<13;seed^=seed>>>17;seed^=seed<<5;return seed>>>0;};
if(name==='scalar'){
    const binary=[(a:number,b:number)=>a+b,(a:number,b:number)=>a-b,Math.imul,
        (a:number,b:number)=>a&b,(a:number,b:number)=>a|b,(a:number,b:number)=>a^b,
        (a:number,b:number)=>a<<b,(a:number,b:number)=>a>>>b,(a:number,b:number)=>a>>b,
        (a:number,b:number)=>(a<<b)|(a>>>-b),(a:number,b:number)=>(a>>>b)|(a<<-b)];
    for(let n=0;n<binary.length;n++)for(let k=0;k<1000;k++){
        const a=random(),b=random();reg(0,a);reg(1,b);assert.equal(invoke(n),binary[n](a,b)>>>0);
    }
    const unary=[(a:number)=>a,(a:number)=>(a<<24)>>24,(a:number)=>(a<<16)>>16,
        (a:number)=>a&255,(a:number)=>a&65535,(a:number)=>~a,(a:number)=>-a,
        (a:number)=>((a&255)<<8)|((a>>>8)&255),
        (a:number)=>((a&255)<<24)|((a&0xff00)<<8)|((a>>>8)&0xff00)|(a>>>24)];
    for(let n=0;n<unary.length;n++)for(let k=0;k<1000;k++){
        const a=random();reg(0,a);assert.equal(invoke(n+binary.length),unary[n](a)>>>0);
    }
}else if(name==='wide'){
    const comparisons=[(a:bigint,b:bigint)=>a===b,(a:bigint,b:bigint)=>a!==b,
        (a:bigint,b:bigint)=>a<b,(a:bigint,b:bigint)=>a>=b,(a:bigint,b:bigint)=>a<=b,(a:bigint,b:bigint)=>a>b,
        (a:bigint,b:bigint)=>a<b,(a:bigint,b:bigint)=>a>=b,(a:bigint,b:bigint)=>a<=b,(a:bigint,b:bigint)=>a>b];
    const values=[0n,1n,0xffffffffffffffffn,0x8000000000000000n,0x7fffffffffffffffn,0x8877665544332211n];
    const inputs=(a:bigint,b:bigint)=>{reg(0,Number(a&0xffffffffn));reg(1,Number(a>>32n));reg(2,Number(b&0xffffffffn));reg(3,Number(b>>32n));};
    for(let c=0;c<comparisons.length;c++)for(let wide=0;wide<2;wide++)for(const a of values)for(const b of values){
        inputs(a,b);const bits=wide?64:32,signed=c>=2&&c<=5;
        const convert=(n:bigint)=>signed?BigInt.asIntN(bits,n):BigInt.asUintN(bits,n);
        assert.equal(invoke(c*2+wide),Number(comparisons[c](convert(a),convert(b))));
    }
    for(let n=0;n<3;n++)for(const a of values)for(const b of values){
        inputs(a,b);invoke(20+n);const expected=n===0?a+b:n===1?a-b:(a&0xffffffffn)*(b&0xffffffffn);
        assert.equal(view.getBigUint64(env,true),BigInt.asUintN(64,expected));
    }
    reg(0,0xdeadbeef);reg(1,0x76543210);assert.equal(invoke(23),0xde3210ef);
    assert.equal(f.b24(env,bank,scratch,0x5000),BigInt.asIntN(64,0xffffffff00000000n));
    view.setUint32(0x7002,0x87654321,true);
    assert.equal(raw.b24(env,bank,scratch,0x7000),BigInt.asIntN(64,0x8765432100000000n));
}else if(name==='branch'){
    for(const branch of [0,1]){
        const incoming=Array.from({length:16},(_,n)=>0xabc000+n);incoming[0]=branch;
        incoming.forEach((value,n)=>reg(n,value));view.setUint32(0x5044,0,true);
        assert.equal(f.b0(env,bank,scratch,0x5000),0x5048n);
        const expected=[...incoming];if(branch)expected[9]=42;
        assert.deepEqual(Array.from({length:16},(_,n)=>view.getUint32(bank+n*4,true)),expected);
    }
    view.setUint32(0x7044,0,true);
    reg(0,0);assert.equal(f.b0(env,bank,scratch,0x7000),0x7048n);
    for(const [a,b] of [[0n,0n],[0n,1n],[0x8000000000000000n,0n],[0xffffffffffffffffn,0xffffffffffffffffn]]){
        const incoming=Array.from({length:16},(_,n)=>0xabc000+n);
        incoming[0]=Number(a&0xffffffffn);incoming[1]=Number(a>>32n);
        incoming[2]=Number(b&0xffffffffn);incoming[3]=Number(b>>32n);
        incoming.forEach((value,n)=>reg(n,value));
        assert.equal(f.b1(env,bank,scratch,0x5000),0x5048n);
        const expected=[...incoming];if(a!==b)expected[9]=42;
        assert.deepEqual(Array.from({length:16},(_,n)=>view.getUint32(bank+n*4,true)),expected);
    }
}else if(name==='control'){
    for(const [fn,offset,count] of [[f.b0,0x64,0],[f.b1,0x34,5],[f.b2,0x7f4,63],[f.b3,0x814,64]] as const)for(const base of [0x5000,0x7000])for(let choice=0;choice<4;choice++){
        const incoming=Array.from({length:16},(_,n)=>0xabc000+n);incoming[0]=choice;
        incoming.forEach((value,n)=>reg(n,value));view.setUint32(base+offset,0,true);
        assert.equal(fn(env,bank,scratch,base),BigInt(base+offset+4));
        const expected=[...incoming];expected[9]=fn===f.b0?(choice<2?11:9):fn===f.b1?5:choice===0?0:count;
        assert.deepEqual(Array.from({length:16},(_,n)=>view.getUint32(bank+n*4,true)),expected);
    }
}else if(name==='helper'){
    let stop=false,codeBase=0x5000;
    helper=(e,fn,b,s,words,mask,pc)=>{
        assert.deepEqual([e,fn,b,s,words,mask,pc],[env,9,bank,scratch,12,0x1555,codeBase+2]);
        for(const r of [0,1,2,3,5,6,7,8,9,10,11,12])assert.equal(view.getUint32(b+r*4,true),0x100+r);
        view.setUint32(b,77,true);view.setUint32(b+4,33,true);return Number(stop);
    };
    const reset=()=>{for(let r=0;r<16;r++)reg(r,0x100+r);reg(14,env);};
    reset();assert.equal(invoke(0),77);assert.equal(view.getUint32(env+4,true),33);
    reset();codeBase=0x7000;assert.equal(f.b0(env,bank,scratch,codeBase),0x123400000000n);codeBase=0x5000;
    reset();stop=true;view.setUint32(env,0xcafebabe,true);assert.equal(f.b0(env,bank,scratch,0x5000),0n);assert.equal(view.getUint32(env,true),0xcafebabe);
}else if(name==='memory'){
    const accesses:unknown[]=[];
    load=(e,a,oi,pc)=>{accesses.push(['load',e,a,oi,pc]);return 0x8877665544332211n;};
    // JS observes an i64 import as signed; compare the native uint64_t bits.
    store=(e,a,value,oi,pc)=>{accesses.push(['store',e,a,BigInt.asUintN(64,value),oi,pc]);};
    reg(1,0x9000);reg(4,0xa000);invoke(0);
    assert.equal(view.getUint32(env,true),0x44332211);assert.equal(view.getBigUint64(env+4,true),0x8877665544332211n);
    assert.deepEqual(accesses,[['load',env,0x9000,0x21,0x5002],['load',env,0x9000,0x33,0x5022],['store',env,0xa000,0x8877665544332211n,0x44,0x5052]]);
    accesses.length=0;reg(14,env);f.b0(env,bank,scratch,0x7000);
    assert.deepEqual(accesses,[['load',env,0x9000,0x21,0x7002],['load',env,0x9000,0x33,0x7022],['store',env,0xa000,0x8877665544332211n,0x44,0x7052]]);
    view.setUint32(env,0x11223344,true);reg(0,0xaabbccdd);reg(1,0x123456ef);invoke(1);
    assert.equal(view.getUint32(env+8,true),0xdd);assert.equal(view.getUint32(env+12,true),0xefccdd44);
}else if(name==='dispatch'){
    table.set(1,raw.b0);table.set(2,raw.b1);
    const dispatcher=new WebAssembly.Instance(new WebAssembly.Module(readFileSync(join(dirname(filename),'dispatch.wasm'))),{env:{memory,table}}).exports.dispatch as (env:number,bank:number,scratch:number,slot:number,codeBase:number)=>bigint;
    let calls=0,limit=50000,onCall=()=>{},expectedPc=0x1008;
    helper=(_e,_fn,_b,_s,_words,_mask,pc)=>{assert.equal(pc,expectedPc);calls++;onCall();return Number(calls>=limit);};
    const invoke=()=>dispatcher(0x6000,0x7000,0x8000,1,0x1006);
    for(const prefix of [0x1000,0x2000]){
        const tb=prefix+0x100;
        view.setUint16(prefix,0x06ff,true);view.setUint32(prefix+2,tb,true);
        view.setUint32(tb+16,1,true);view.setUint32(tb+24,prefix,true);
        view.setUint32(prefix+0x1a,prefix-(prefix+0x1e),true);
    }
    assert.equal(invoke(),0n);assert.equal(calls,limit);
    for(const offset of [0,8,16])assert.equal(view.getBigUint64(0x9000+offset,true),BigInt(limit-1));
    calls=0;view.setUint32(0x1110,0,true);assert.equal(invoke(),0x1000n);assert.equal(calls,1);
    view.setUint8(0x1000,42);view.setUint32(0x1110,1,true);calls=0;
    assert.equal(invoke(),0x1000n);assert.equal(calls,1);
    view.setUint8(0x1000,255);calls=0;onCall=()=>{if(calls===123)view.setUint32(0x1110,0,true);};
    assert.equal(invoke(),0x1000n);assert.equal(calls,123);
    view.setUint32(0x1110,2,true);view.setUint32(0x1118,0x3000,true);view.setUint32(0x3008,0x12345678,true);onCall=()=>{};calls=0;
    assert.equal(invoke(),0x1234567800000000n);assert.equal(calls,1);
    // The same function slot runs at two current allocations. Each edge uses
    // that TB's code base for both its precise helper PC and its link word.
    view.setUint32(0x1110,1,true);view.setUint32(0x1118,0x1000,true);
    view.setUint32(0x101a,0x2000-0x101e,true);view.setUint32(0x201a,0x1000-0x201e,true);
    calls=0;onCall=()=>{expectedPc=expectedPc===0x1008?0x2008:0x1008;};
    assert.equal(invoke(),0n);assert.equal(calls,limit);
}else{throw Error(`unknown test ${name}`);}
console.log(JSON.stringify({passed:true,test:name}));
