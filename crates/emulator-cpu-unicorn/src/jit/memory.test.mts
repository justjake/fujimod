// Differential direct-TLB execution against independently modeled byte values.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {dirname,join} from 'node:path';
import {withSource,type Fn} from './source.test.mts';
const filename=process.argv[2];assert(filename);
const memory=new WebAssembly.Memory({initial:4}),table=new WebAssembly.Table({initial:1,element:'anyfunc'}),v=new DataView(memory.buffer);
const env=0x1000,regs=0x2000,uc=0x3000,tlb=0x4000,ram=0x10000,guest=0x80000200;
let calls:unknown[][]=[];
let onLoad=()=>0x0123456789abcdefn;
const imports={env:{memory,table,load:(...args:number[])=>{calls.push(['load',...args]);return onLoad();},
    store:(...args:(number|bigint)[])=>{calls.push(['store',...args]);},helper:()=>{throw Error('unexpected helper');}}};
const loadModule=(path:string)=>{const module=new WebAssembly.Module(readFileSync(path));return withSource(module,new WebAssembly.Instance(module,imports),memory);};
const functions=loadModule(filename),alignment=loadModule(join(dirname(filename),'alignment.wasm')),reload=loadModule(join(dirname(filename),'reload.wasm'));
function tag(address:number,mode:number,flags=0){
    const desc=env+256+mode*8,at=v.getUint32(desc+4,true)+(((address>>>10)<<5)&v.getUint32(desc,true));
    v.setUint32(at,(address&-1024)|flags,true);v.setUint32(at+4,(address&-1024)|flags,true);
    v.setUint32(at+24,(ram-(address&-1024))>>>0,true);return at;
}
function reset(){
    new Uint8Array(memory.buffer).fill(0);calls=[];onLoad=()=>0x0123456789abcdefn;
    v.setUint32(env+128,uc,true);v.setUint8(uc,1);
    for(let mode=0;mode<2;mode++){v.setUint32(env+256+mode*8,96,true);v.setUint32(env+260+mode*8,tlb+mode*0x100,true);}
    for(let i=0;i<8;i++){v.setUint32(tlb+i*32,0xffffffff,true);v.setUint32(tlb+i*32+4,0xffffffff,true);}
    tag(guest,0);
}
function run(fn:Fn,address=guest,value=0x0123456789abcdefn){
    calls=[];v.setUint32(regs,Number(value&0xffffffffn),true);v.setUint32(regs+4,Number(value>>32n),true);
    v.setUint32(regs+8,address,true);v.setUint32(regs+56,env,true);
    assert.equal(fn(env,regs,0x6000,0x5000),0x123400000000n);
    return v.getBigUint64(env+160,true);
}
let n=0,cases=0;
for(let shift=0;shift<=3;shift++)for(const signed of [false,true])for(const mode of [0,1])for(const swapped of [false,true]){
    reset();tag(guest,mode);const at=ram+(guest&1023),size=2**shift;
    for(let i=0;i<size;i++)v.setUint8(at+i,(0xf1+i*7)&255);
    const expected=shift===0?BigInt(signed?v.getInt8(at):v.getUint8(at)):
        shift===1?BigInt(signed?v.getInt16(at,!swapped):v.getUint16(at,!swapped)):
        shift===2?BigInt(signed?v.getInt32(at,!swapped):v.getUint32(at,!swapped)):v.getBigUint64(at,!swapped);
    assert.equal(run(functions[`b${n++}`]),BigInt.asUintN(64,expected));assert.equal(calls.length,0);cases++;
    reset();tag(guest,mode);run(functions[`b${n++}`]);assert.equal(calls.length,0);
    for(let i=0;i<size;i++)assert.equal(v.getUint8(at+i),Number((0x0123456789abcdefn>>BigInt((swapped?size-1-i:i)*8))&255n));cases++;
}
const load32=functions.b32,store32=functions.b33,load64=functions.b48,store64=functions.b49;
reset();run(store32,guest+1);assert.equal(calls.length,0);
run(alignment.b0,guest+1);assert.equal(calls.length,1);
run(load64,(guest&-1024)+1020);assert.equal(calls.length,1);
run(store64,0xfffffffc);assert.equal(calls.length,1);
for(const flag of [8,16,32,64,128,256,512]){
    reset();tag(guest,0,flag);run(load32);assert.equal(calls.length,1);run(store32);assert.equal(calls.length,1);
}
reset();run(load32,guest+1024);assert.equal(calls.length,1);
for(const offset of [4,8,12]){
    reset();v.setUint8(uc,0);v.setUint32(uc+offset,1,true);run(load32);assert.equal(calls.length,1);
    v.setUint8(uc,1);run(load32);assert.equal(calls.length,0);
}
reset();v.setUint32(env+256,32,true);v.setUint32(env+260,tlb+0x400,true);tag(guest,0);
v.setUint32(ram+(guest&1023),0xaabbccdd,true);assert.equal(run(load32),0xaabbccddn);assert.equal(calls.length,0);
const entry=tag(guest,0);v.setUint32(entry+24,(ram+1024-(guest&-1024))>>>0,true);v.setUint32(ram+1024+(guest&1023),42,true);
assert.equal(run(load32),42n);assert.equal(calls.length,0);
// A slow first access replaces the table before a second access in one body.
reset();v.setUint32(regs+12,guest,true);onLoad=()=>{
    v.setUint32(env+256,32,true);v.setUint32(env+260,tlb+0x400,true);tag(guest,0);
    v.setUint32(ram+(guest&1023),0xaabbccdd,true);return 0n;
};
assert.equal(run(reload.b0,guest+1024),0xaabbccddn);assert.equal(calls.length,1);
console.log(JSON.stringify({passed:true,directCases:cases,flags:true,hooks:true,alignment:true,crossPage:true,dynamicTlb:true,alias:true}));
