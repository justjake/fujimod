"""SHA-bound module placement checks, including original startup stacks.

Recovery recipe region/BSS exclusions alone miss stacks initialized by ARM
entry57978. This is a static exclusion check; runtime byte checks remain needed.
"""
import hashlib,struct

STOCK_SHA='ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172'
SETUPS=[('irq',0x579b0),('abort',0x579d0),('undefined',0x579f0),
        ('system',0x57a10),('supervisor',0x57a30)]

def startup_stacks(stock):
    if hashlib.sha256(stock).hexdigest()!=STOCK_SHA:raise ValueError('Unknown X-Pro2 startup image')
    result=[]
    for mode,pc in SETUPS:
        first,second=struct.unpack_from('<2I',stock,pc)
        assert first&0xfffff000==0xe59f1000 and second&0xfffff000==0xe59f2000
        base=struct.unpack_from('<I',stock,pc+8+(first&0xfff))[0]
        size_pointer=struct.unpack_from('<I',stock,pc+12+(second&0xfff))[0]
        size=struct.unpack_from('<I',stock,size_pointer)[0]
        assert size==0x800
        result.append({'name':mode,'start':base,'end':base+2*size,
                       'cores':2,'bytes_per_core':size,'setup_pc':pc,'size_pointer':size_pointer})
    return result

def check_module_allocation(stock,plan,start,end):
    if not 0<=start<end<=plan['size']:raise ValueError('Module outside resident image')
    if any(stock[start:end]):raise ValueError('Module allocation is not stock zero space')
    blocked=[]
    for region in [*plan['regions'],*plan.get('bss',[])]:
        size=region['size'] if 'size' in region else region['source']['decoded_size']
        blocked.append((region['address'],region['address']+size,region['name']))
    stacks=startup_stacks(stock)
    # Reserve the entire early-stack envelope, including its inter-mode gap.
    blocked.append((min(s['start'] for s in stacks),max(s['end'] for s in stacks),'early CPU stacks'))
    blocked.extend([(0x103000,0x103600,'resident entry stub'),
                    (0x103800,0x103920,'opacity helper'),(0x103b00,0x103b10,'opacity state'),
                    (0x104000,0x1e0000,'resident patch table and source blobs')])
    for a,b,name in blocked:
        if start<b and a<end:raise ValueError(f'Module overlaps {name}: {a:#x}..{b:#x}')
    return {'address':hex(start),'end':hex(end),'bytes':end-start,
            'startup_stacks':stacks,'static_exclusions_passed':True,
            'limitations':['Runtime virtual mapping and byte preservation still require verification']}
