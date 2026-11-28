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
    DYNARMIC_EXIT_REASON_REPLAY = 10,
} DynarmicExitReason;

typedef struct DynarmicCallbacks {
    void* user_data;
    void* inline_read32;
    uint32_t (*batch_instruction)(uint32_t);
    bool (*is_shift_count_loop)(uint32_t, uint32_t, uint32_t);
    void (*guard_transfer)(void*, uint32_t, uint32_t);
    bool (*read_code)(void*, uint32_t, uint32_t*);
    uint64_t (*read)(void*, uint32_t, uint32_t);
    uint32_t (*timed_read32)(void*, uint32_t, uint64_t);
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
    uint32_t cpacr;
} DynarmicMemoryControl;

typedef struct DynarmicInstance DynarmicInstance;

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks, bool rprofile);
void dynarmic_destroy(DynarmicInstance* inst);

void dynarmic_set_translation_context(DynarmicInstance* inst, uint32_t value, bool vfp_enabled);
void dynarmic_observe_stores(DynarmicInstance* inst, bool enabled);
void dynarmic_read_gprs(DynarmicInstance* inst, uint32_t* values);
void dynarmic_read_snapshot(DynarmicInstance* inst, uint32_t* ext_regs, uint32_t* cp15, uint32_t* cache_ids);
void dynarmic_write_snapshot(DynarmicInstance* inst, const uint32_t* ext_regs, const uint32_t* cp15, const uint32_t* cache_ids);
uint8_t** dynarmic_data_pages(DynarmicInstance* inst);
DynarmicExitReason dynarmic_run(DynarmicInstance* inst, uint64_t budget, uint64_t* executed);
DynarmicExitReason dynarmic_step(DynarmicInstance* inst, uint64_t* executed);
void dynarmic_abort_access(DynarmicInstance* inst);
void dynarmic_memory_control(DynarmicInstance* inst, DynarmicMemoryControl* control);
void dynarmic_stop(DynarmicInstance* inst);

void dynarmic_clear_cache(DynarmicInstance* inst);
void dynarmic_invalidate_cache(DynarmicInstance* inst, uint32_t vaddr, size_t length);

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
