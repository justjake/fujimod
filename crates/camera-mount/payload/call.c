#include <stdint.h>
__attribute__((section(".text.entry"),used))
unsigned char query_call(uint32_t argument, uint32_t *output) {
    uint32_t (*call)(uint32_t)=(void *)(uintptr_t)output[1];
    output[0]=call(argument);
    return 0;
}
