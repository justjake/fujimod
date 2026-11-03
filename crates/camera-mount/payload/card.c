/* Owned asynchronous file service. Stock entrypoints and all shared storage
 * arrive in the request; this position-independent image contains no firmware
 * bytes or addresses. Callbacks run on the stock storage task. */
#include <stdint.h>
typedef uint32_t u32;
typedef void (*cb1)(int);
typedef void (*cb2)(int, int);
enum { OPEN, READ, WRITE, CLOSE, ABORT, SEEK, DRIVE, SEND, TASK,
       PARSE, READY, FIND_FAT, FIND_EXFAT, SCAN_FAT, SCAN_EXFAT, VOLUMES, TRANSLATE,
       CPU_CALL, INSTRUCTION_INVALIDATE, INSTRUCTION_BARRIER, FUNCTIONS };
struct service {
    u32 api[16], capacity, request, state, status, count;
    char path[64];
    u32 operations, enqueue, handle, source, functions[FUNCTIONS];
    u32 directory_started, skip, include_directories;
    unsigned char first[60], command[12], cursor[136];
    u32 range_offset, range_size, range_status;
};
static struct service *active = (void *)1;
/* Both the query task and asynchronous storage callbacks use the uncached
 * view of this cell. Each allocated image has its own cell. */
static struct service *current(void) {
    return *(struct service *volatile *)((uintptr_t)&active | 0x40000000u);
}
static void copy(void *to, const void *from, u32 n) {
    unsigned char *a = to; const unsigned char *b = from;
    while (n--) *a++ = *b++;
}
static void zero(void *to, u32 n) { unsigned char *p = to; while(n--) *p++ = 0; }
static u32 word(const void *p) { u32 n; copy(&n, p, 4); return n; }
static void done(int status) {
    struct service *s = current(); s->status = status;
    __asm__ volatile("dmb sy" ::: "memory");
    s->state = status ? 3 : 2;
}
static void opened(int status, int handle) {
    if (!status) current()->handle = handle;
    done(status);
}
static void transferred(int status, int count) { current()->count = count; done(status); }
static void closed(int status) { current()->handle = UINT32_MAX; done(status); }
static void range_closed(int status) {
    struct service *s=current(); s->handle=UINT32_MAX;
    done(status ? status : s->range_status);
}
static void range_close(int status) {
    struct service *s=current(); s->range_status=status;
    ((int (*)(cb1,int,u32,int))(uintptr_t)s->functions[CLOSE])(range_closed,s->handle,0,0);
}
static void range_read(int status,int count) {
    current()->count=count; range_close(status);
}
static void range_seek(int status) {
    struct service *s=current();
    if(status) { range_close(status); return; }
    ((int (*)(cb2,int,int,void *))(uintptr_t)s->functions[READ])(
        range_read,s->handle,s->range_size,(void *)(uintptr_t)s->api[9]);
}
static void range_open(int status,int handle) {
    struct service *s=current();
    if(status) { done(status); return; }
    s->handle=handle;
    ((void (*)(cb1,int,u32,int,int))(uintptr_t)s->functions[SEEK])(
        range_seek,handle,s->range_offset,0,0);
}
static int translate(int status) {
    struct service *s = current();
    return ((int (*)(u32,int))(uintptr_t)s->functions[TRANSLATE])(0x2c06, status);
}
static void listed(int initial_status) {
    struct service *s = current();
    if (initial_status && initial_status != 6) { done(initial_status); return; }
    /* Stock's find-first packet enters the storage task. The same FAT/exFAT
     * cursor primitives then enumerate directories as well as regular files. */
    unsigned char *cursor = s->cursor + 0x44;
    int status;
    if (!s->directory_started) {
        zero(s->cursor, sizeof s->cursor);
        status = ((int (*)(const char *,void *,int,int))(uintptr_t)s->functions[PARSE])(
            s->path,s->cursor,1,0);
        if (status) { done(8); return; }
        int device = word(s->cursor + 0x48);
        status = ((int (*)(int))(uintptr_t)s->functions[READY])(device);
        if (status) { done(translate(status)); return; }
        u32 volumes = *(volatile u32 *)(uintptr_t)s->functions[VOLUMES];
        int exfat = *(volatile unsigned char *)(uintptr_t)(volumes + device * 0xa4 + 0x4b) == 3;
        status = exfat
            ? ((int (*)(void *,int))(uintptr_t)s->functions[FIND_EXFAT])(s->cursor,0)
            : ((int (*)(void *))(uintptr_t)s->functions[FIND_FAT])(s->cursor);
        if (status) { done(translate(status == 0x10a ? 8 : 6)); return; }
        cursor[2] = 2;
        s->directory_started = 1;
    }
    for (;;) {
        int device = word(cursor + 4);
        u32 volumes = *(volatile u32 *)(uintptr_t)s->functions[VOLUMES];
        int exfat = *(volatile unsigned char *)(uintptr_t)(volumes + device * 0xa4 + 0x4b) == 3;
        status = ((int (*)(void *))(uintptr_t)s->functions[exfat ? SCAN_EXFAT : SCAN_FAT])(cursor);
        if (status) { done(status == 0x10a ? 0 : translate(status == 11 ? 11 : 7)); return; }
        unsigned char *entry = cursor + 0x24;
        if (!(entry[11] & 8) && (s->include_directories || !(entry[11] & 0x10))) {
            if (s->skip) --s->skip;
            else if (s->count + 24 <= s->capacity) {
                unsigned char *record = (void *)(uintptr_t)(s->api[9] + s->count);
                zero(record,24); copy(record,entry+28,4);
                u32 n = 4;
                for (u32 i=0;i<11;i++) {
                    if (i==8 && entry[i]!=' ') record[n++]='.';
                    if (entry[i]!=' ') record[n++]=entry[i];
                }
                record[23] = s->include_directories ? entry[11] : 0;
                s->count += 24;
            }
        }
        *(u32 *)(cursor + 0x14) += 1;
    }
}
__attribute__((section(".text.entry"),used,visibility("default")))
unsigned char card_service(struct service *s, u32 *output) {
    (void)output;
    *(struct service *volatile *)((uintptr_t)&active | 0x40000000u) = s;
    u32 argument = s->request, op = argument & 255, size = argument >> 8;
    if (s->state == 1) { s->enqueue = 1; return 0; }
    s->enqueue=0; s->operations++; s->state=1; s->request=0;
    if (op==13) {
        /* Only stock code runs on CPU1 during initialization. Invalidate its
         * instruction cache before any storage callback enters this image. */
        typedef void (*submit)(u32,u32,const void *,u32);
        ((submit)(uintptr_t)s->functions[CPU_CALL])(1,s->functions[INSTRUCTION_INVALIDATE],0,0);
        ((submit)(uintptr_t)s->functions[CPU_CALL])(1,s->functions[INSTRUCTION_BARRIER],0,0);
        done(0); return 0;
    }
    if (op==1 || op==5 || op==7 || op==8 || op==9 || op==10 || op==14)
        s->path[0] = ((int (*)(void))(uintptr_t)s->functions[DRIVE])();
    if ((op==1 || op==5 || op==7 || op==8 || op==9 || op==10 || op==14) && s->handle!=UINT32_MAX) {
        done(-16); return 0;
    }
    if ((op==2 || op==3 || op==4 || op==6 || op==11 || op==12) && s->handle==UINT32_MAX) {
        done(-16); return 0;
    }
    if (op==14) {
        if (!size || size>s->capacity) { done(-22); return 0; }
        s->range_offset=word((void *)(uintptr_t)s->api[9]);
        s->range_size=size; s->range_status=0; s->count=0;
        ((int (*)(cb2,const char *,int))(uintptr_t)s->functions[OPEN])(range_open,s->path,0);
    } else if (op==1 || op==5 || op==9) {
        ((int (*)(cb2,const char *,int))(uintptr_t)s->functions[OPEN])(
            opened,s->path,op==1 ? 0 : op==5 ? 2 : 1);
    } else if (op==2 || op==6) {
        if (size>s->capacity) size=s->capacity;
        void *buffer=(void *)(uintptr_t)(op==6 && s->source ? s->source : s->api[9]);
        ((int (*)(cb2,int,int,void *))(uintptr_t)s->functions[op==2 ? READ : WRITE])(
            transferred,s->handle,size,buffer);
    } else if (op==3 || op==12) {
        u32 limit=op==12 ? word((void *)(uintptr_t)s->api[9]) : 0;
        if (op==12 && !limit) done(-22);
        else ((int (*)(cb1,int,u32,int))(uintptr_t)s->functions[CLOSE])(closed,s->handle,limit,0);
    } else if (op==4) {
        ((int (*)(cb1,int))(uintptr_t)s->functions[ABORT])(closed,s->handle);
    } else if (op==11) {
        ((void (*)(cb1,int,u32,int,int))(uintptr_t)s->functions[SEEK])(
            done,s->handle,word((void *)(uintptr_t)s->api[9]),0,0);
    } else if (op==7 || op==8 || op==10) {
        zero(s->first,sizeof s->first);
        u32 code=0x2c0a, bytes=52;
        *(cb1 *)s->first=done;
        if (op==7) copy(s->first+4,s->path,48);
        else {
            code=0x2c06; bytes=60; s->directory_started=0; s->skip=size;
            s->include_directories=op==10; s->count=0;
            *(cb1 *)s->first=listed;
            *(u32 *)(s->first+4)=s->api[9];
            /* The packet is an entry ticket to the storage task; listed()
             * resolves the full requested path with its owned cursor. A root
             * ticket avoids imposing the stock packet's 32-byte folder limit
             * on the helper's 48-byte namespace. */
            s->first[8]=s->path[0];s->first[9]=':';s->first[10]='\\';
            s->first[40]='*';s->first[41]='.';s->first[42]='*';
        }
        u32 sender=((int (*)(void))(uintptr_t)s->functions[TASK])();
        *(u32 *)s->command=code | (sender & 255)<<16 | 0x69u<<24;
        *(u32 *)(s->command+4)=bytes;
        *(u32 *)(s->command+8)=(uintptr_t)s->first;
        int status=((int (*)(void *))(uintptr_t)s->functions[SEND])(s->command);
        if(status) done(status);
    } else done(-22);
    return 0;
}
