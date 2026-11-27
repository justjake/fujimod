#pragma once
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef enum DynarmicExitReason {
    DYNARMIC_EXIT_REASON_TIMEOUT = 0,
    DYNARMIC_EXIT_REASON_STOPPED = 1,
    DYNARMIC_EXIT_REASON_WFI = 2,
    DYNARMIC_EXIT_REASON_WFE = 3,
    DYNARMIC_EXIT_REASON_SEV = 4,
    DYNARMIC_EXIT_REASON_SVC = 5,
    DYNARMIC_EXIT_REASON_UNDEFINED = 6,
    DYNARMIC_EXIT_REASON_NO_EXECUTE = 7,
    DYNARMIC_EXIT_REASON_BREAKPOINT = 8,
    DYNARMIC_EXIT_REASON_ERROR = 9,
} DynarmicExitReason;

typedef struct DynarmicCallbacks {
    void* user_data;
    bool (*read_code)(void*, uint32_t, uint32_t*);
    uint64_t (*read)(void*, uint32_t, uint32_t);
    void (*write)(void*, uint32_t, uint32_t, uint64_t);
} DynarmicCallbacks;

typedef struct DynarmicMemoryControl {
    uint32_t sctlr;
    uint32_t ttbr0;
    uint32_t ttbr1;
    uint32_t ttbcr;
    uint32_t dacr;
    uint32_t fcseidr;
    uint32_t cpsr;
} DynarmicMemoryControl;

typedef struct DynarmicInstance DynarmicInstance;

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks);
void dynarmic_destroy(DynarmicInstance* inst);

DynarmicExitReason dynarmic_step(DynarmicInstance* inst, uint64_t* executed);
void dynarmic_abort_access(DynarmicInstance* inst);
void dynarmic_memory_control(DynarmicInstance* inst, DynarmicMemoryControl* control);
void dynarmic_stop(DynarmicInstance* inst);

void dynarmic_clear_cache(DynarmicInstance* inst);
void dynarmic_invalidate_cache(DynarmicInstance* inst, uint32_t vaddr, size_t length);

void dynarmic_read_gprs(DynarmicInstance* inst, uint32_t* values);
uint32_t dynarmic_get_reg(DynarmicInstance* inst, uint32_t reg);
void dynarmic_set_reg(DynarmicInstance* inst, uint32_t reg, uint32_t val);

uint32_t dynarmic_get_cpsr(DynarmicInstance* inst);
void dynarmic_set_cpsr(DynarmicInstance* inst, uint32_t val);

uint32_t dynarmic_get_fpscr(DynarmicInstance* inst);
void dynarmic_set_fpscr(DynarmicInstance* inst, uint32_t val);


void dynarmic_set_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2, uint32_t val);
uint32_t dynarmic_get_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2);
void dynarmic_set_cache_ids(DynarmicInstance* inst, uint32_t l1d, uint32_t l1i, uint32_t l2);

#ifdef __cplusplus
}
#endif
