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
    uint32_t (*read_code)(void* user_data, uint32_t vaddr);
    uint8_t (*read_8)(void* user_data, uint32_t vaddr);
    uint16_t (*read_16)(void* user_data, uint32_t vaddr);
    uint32_t (*read_32)(void* user_data, uint32_t vaddr);
    uint64_t (*read_64)(void* user_data, uint32_t vaddr);
    void (*write_8)(void* user_data, uint32_t vaddr, uint8_t val);
    void (*write_16)(void* user_data, uint32_t vaddr, uint16_t val);
    void (*write_32)(void* user_data, uint32_t vaddr, uint32_t val);
    void (*write_64)(void* user_data, uint32_t vaddr, uint64_t val);
    void (*call_svc)(void* user_data, uint32_t swi);
    void (*exception_raised)(void* user_data, uint32_t pc, uint32_t exception);
    uint32_t (*cp15_read)(void* user_data, unsigned opc1, unsigned crn, unsigned crm, unsigned opc2);
    void (*cp15_write)(void* user_data, unsigned opc1, unsigned crn, unsigned crm, unsigned opc2, uint32_t val);
} DynarmicCallbacks;

typedef struct DynarmicInstance DynarmicInstance;

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks);
void dynarmic_destroy(DynarmicInstance* inst);

DynarmicExitReason dynarmic_run(DynarmicInstance* inst, uint64_t instructions, uint64_t* executed_instructions);
DynarmicExitReason dynarmic_step(DynarmicInstance* inst);
void dynarmic_stop(DynarmicInstance* inst);

void dynarmic_clear_cache(DynarmicInstance* inst);
void dynarmic_invalidate_cache(DynarmicInstance* inst, uint32_t vaddr, size_t length);

uint32_t dynarmic_get_reg(DynarmicInstance* inst, uint32_t reg);
void dynarmic_set_reg(DynarmicInstance* inst, uint32_t reg, uint32_t val);

uint32_t dynarmic_get_cpsr(DynarmicInstance* inst);
void dynarmic_set_cpsr(DynarmicInstance* inst, uint32_t val);

uint32_t dynarmic_get_fpscr(DynarmicInstance* inst);
void dynarmic_set_fpscr(DynarmicInstance* inst, uint32_t val);

void dynarmic_map_page(DynarmicInstance* inst, uint32_t page_index, uint8_t* host_ptr);
void dynarmic_unmap_page(DynarmicInstance* inst, uint32_t page_index);
void dynarmic_map_pages(DynarmicInstance* inst, uint32_t start_page, uint32_t num_pages, uint8_t* host_ptr);
void dynarmic_unmap_pages(DynarmicInstance* inst, uint32_t start_page, uint32_t num_pages);

void dynarmic_set_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2, uint32_t val);
uint32_t dynarmic_get_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2);
void dynarmic_set_cache_ids(DynarmicInstance* inst, uint32_t l1d, uint32_t l1i, uint32_t l2);

#ifdef __cplusplus
}
#endif
