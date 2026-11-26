#include "dynarmic_ffi.h"
#include "dynarmic/interface/A32/a32.h"
#include "dynarmic/interface/A32/config.h"
#include "dynarmic/interface/A32/coprocessor.h"
#include "dynarmic/interface/exclusive_monitor.h"
#include <memory>
#include <array>
#include <cstring>
#include <atomic>
#if defined(__APPLE__)
#include <sys/mman.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>
#endif

using Dynarmic::ExclusiveMonitor;
using namespace Dynarmic::A32;

struct DynarmicInstance;

class CP15Impl final : public Coprocessor {
public:
    explicit CP15Impl(DynarmicInstance* inst) : instance(inst) {
        regs.fill(0);
        regs[encode(0, 0, 0, 0)] = 0x410fc075; // MIDR (ARM Cortex-A7 r0p5)
        regs[encode(0, 0, 0, 1)] = 0x8444c004; // CTR
        regs[encode(0, 0, 0, 5)] = 0x80000000; // MPIDR (Multiprocessor Affinity, Core 0 default)
        regs[encode(0, 1, 0, 0)] = 0x00c50078; // SCTLR
        regs[encode(0, 1, 0, 1)] = 0x00000040; // ACTLR (SMP enabled)
        regs[encode(0, 1, 0, 2)] = 0x00f00000; // CPACR (full access to CP10/CP11 VFP/NEON)
        regs[encode(4, 15, 0, 0)] = 0xffff0000; // CBAR (Configuration Base Address)
        regs[encode(1, 0, 0, 0)] = cache_ids[0]; // CCSIDR default
        regs[encode(1, 0, 0, 1)] = 0x0a200023;   // CLIDR (Cortex-A7: L1 split I/D, L2 unified, LoC=2)
    }

    static constexpr size_t encode(unsigned opc1, unsigned crn, unsigned crm, unsigned opc2) {
        return ((opc1 & 7) << 11) | ((crn & 15) << 7) | ((crm & 15) << 3) | (opc2 & 7);
    }

    static std::uint64_t WriteCSSELR(void* user_arg, std::uint32_t val, std::uint32_t) {
        auto self = static_cast<CP15Impl*>(user_arg);
        self->Write(2, 0, 0, 0, val);
        uint32_t sel = val & 7;
        if (sel < 3) {
            self->Write(1, 0, 0, 0, self->cache_ids[sel]);
        }
        return 0;
    }

    std::optional<Callback> CompileInternalOperation(bool, unsigned, CoprocReg, CoprocReg, CoprocReg, unsigned) override {
        return std::nullopt;
    }

    CallbackOrAccessOneWord CompileSendOneWord(bool, unsigned opc1, CoprocReg CRn, CoprocReg CRm, unsigned opc2) override {
        unsigned crn = static_cast<unsigned>(CRn);
        unsigned crm = static_cast<unsigned>(CRm);
        if (opc1 == 2 && crn == 0 && crm == 0 && opc2 == 0) {
            return Callback{ &WriteCSSELR, this };
        }
        size_t key = encode(opc1, crn, crm, opc2);
        return &regs[key];
    }

    CallbackOrAccessTwoWords CompileSendTwoWords(bool, unsigned, CoprocReg) override {
        return std::monostate{};
    }

    CallbackOrAccessOneWord CompileGetOneWord(bool, unsigned opc1, CoprocReg CRn, CoprocReg CRm, unsigned opc2) override {
        unsigned crn = static_cast<unsigned>(CRn);
        unsigned crm = static_cast<unsigned>(CRm);
        size_t key = encode(opc1, crn, crm, opc2);
        return &regs[key];
    }

    CallbackOrAccessTwoWords CompileGetTwoWords(bool, unsigned, CoprocReg) override {
        return std::monostate{};
    }

    std::optional<Callback> CompileLoadWords(bool, bool, CoprocReg, std::optional<std::uint8_t>) override {
        return std::nullopt;
    }

    std::optional<Callback> CompileStoreWords(bool, bool, CoprocReg, std::optional<std::uint8_t>) override {
        return std::nullopt;
    }

    uint32_t Read(unsigned opc1, unsigned crn, unsigned crm, unsigned opc2) const {
        return regs[encode(opc1, crn, crm, opc2)];
    }

    void Write(unsigned opc1, unsigned crn, unsigned crm, unsigned opc2, uint32_t val) {
        regs[encode(opc1, crn, crm, opc2)] = val;
    }

    void SetCacheIds(uint32_t l1d, uint32_t l1i, uint32_t l2) {
        cache_ids = {l1d, l1i, l2};
        regs[encode(1, 0, 0, 0)] = l1d;
    }

    DynarmicInstance* instance;
    std::array<uint32_t, 16384> regs;
    std::array<uint32_t, 3> cache_ids{0x7000001a, 0x20000009, 0x7000003a};
};

class UserCallbacksImpl final : public UserCallbacks {
public:
    explicit UserCallbacksImpl(DynarmicCallbacks cb, DynarmicInstance* inst)
        : callbacks(cb), instance(inst) {}

    std::uint8_t* ram_ptr(VAddr vaddr, size_t size) const;
    std::optional<std::uint32_t> MemoryReadCode(VAddr vaddr) override;

    std::uint8_t MemoryRead8(VAddr vaddr) override {
        if (auto* p = ram_ptr(vaddr, 1)) { std::uint8_t v; std::memcpy(&v, p, sizeof(v)); return v; }
        return callbacks.read_8(callbacks.user_data, vaddr);
    }
    std::uint16_t MemoryRead16(VAddr vaddr) override {
        if (auto* p = ram_ptr(vaddr, 2)) { std::uint16_t v; std::memcpy(&v, p, sizeof(v)); return v; }
        return callbacks.read_16(callbacks.user_data, vaddr);
    }
    std::uint32_t MemoryRead32(VAddr vaddr) override {
        if (auto* p = ram_ptr(vaddr, 4)) { std::uint32_t v; std::memcpy(&v, p, sizeof(v)); return v; }
        return callbacks.read_32(callbacks.user_data, vaddr);
    }
    std::uint64_t MemoryRead64(VAddr vaddr) override {
        if (auto* p = ram_ptr(vaddr, 8)) { std::uint64_t v; std::memcpy(&v, p, sizeof(v)); return v; }
        return callbacks.read_64(callbacks.user_data, vaddr);
    }

    void MemoryWrite8(VAddr vaddr, std::uint8_t value) override {
        if (auto* p = ram_ptr(vaddr, 1)) { std::memcpy(p, &value, sizeof(value)); return; }
        callbacks.write_8(callbacks.user_data, vaddr, value);
    }
    void MemoryWrite16(VAddr vaddr, std::uint16_t value) override {
        if (auto* p = ram_ptr(vaddr, 2)) { std::memcpy(p, &value, sizeof(value)); return; }
        callbacks.write_16(callbacks.user_data, vaddr, value);
    }
    void MemoryWrite32(VAddr vaddr, std::uint32_t value) override {
        if (auto* p = ram_ptr(vaddr, 4)) { std::memcpy(p, &value, sizeof(value)); return; }
        callbacks.write_32(callbacks.user_data, vaddr, value);
    }
    void MemoryWrite64(VAddr vaddr, std::uint64_t value) override {
        if (auto* p = ram_ptr(vaddr, 8)) { std::memcpy(p, &value, sizeof(value)); return; }
        callbacks.write_64(callbacks.user_data, vaddr, value);
    }

    bool MemoryWriteExclusive8(VAddr vaddr, std::uint8_t value, std::uint8_t expected) override;
    bool MemoryWriteExclusive16(VAddr vaddr, std::uint16_t value, std::uint16_t expected) override;
    bool MemoryWriteExclusive32(VAddr vaddr, std::uint32_t value, std::uint32_t expected) override;
    bool MemoryWriteExclusive64(VAddr vaddr, std::uint64_t value, std::uint64_t expected) override;

    void InterpreterFallback(VAddr, size_t) override {}

    void CallSVC(std::uint32_t swi) override;
    void ExceptionRaised(VAddr pc, Exception exception) override;

    void AddTicks(std::uint64_t ticks) override {
        executed_ticks += ticks;
        if (remaining_ticks >= ticks) {
            remaining_ticks -= ticks;
        } else {
            remaining_ticks = 0;
        }
    }

    std::uint64_t GetTicksRemaining() override {
        return remaining_ticks;
    }

    void ResetTicks(std::uint64_t ticks) {
        remaining_ticks = ticks;
        executed_ticks = 0;
    }

    std::uint64_t ExecutedTicks() const {
        return executed_ticks;
    }

private:
    DynarmicCallbacks callbacks;
    DynarmicInstance* instance;
    std::uint64_t remaining_ticks = 0;
    std::uint64_t executed_ticks = 0;
};

struct DynarmicInstance {
    DynarmicCallbacks callbacks;
    std::unique_ptr<ExclusiveMonitor> monitor;
    std::unique_ptr<std::array<std::uint8_t*, UserConfig::NUM_PAGE_TABLE_ENTRIES>> page_table;
    std::unique_ptr<UserCallbacksImpl> callbacks_impl;
    std::shared_ptr<CP15Impl> cp15_impl;
    std::unique_ptr<Jit> jit;
    uint8_t* fastmem_base = nullptr;
    DynarmicExitReason exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    std::atomic<bool> stop_requested{false};
};

bool UserCallbacksImpl::MemoryWriteExclusive8(VAddr vaddr, std::uint8_t value, std::uint8_t expected) {
    if (auto* p = ram_ptr(vaddr, 1)) {
        std::uint8_t cur; std::memcpy(&cur, p, sizeof(cur));
        if (cur != expected) return false;
        std::memcpy(p, &value, sizeof(value));
        return true;
    }
    MemoryWrite8(vaddr, value);
    return true;
}

bool UserCallbacksImpl::MemoryWriteExclusive16(VAddr vaddr, std::uint16_t value, std::uint16_t expected) {
    if (auto* p = ram_ptr(vaddr, 2)) {
        std::uint16_t cur; std::memcpy(&cur, p, sizeof(cur));
        if (cur != expected) return false;
        std::memcpy(p, &value, sizeof(value));
        return true;
    }
    MemoryWrite16(vaddr, value);
    return true;
}

bool UserCallbacksImpl::MemoryWriteExclusive32(VAddr vaddr, std::uint32_t value, std::uint32_t expected) {
    if (auto* p = ram_ptr(vaddr, 4)) {
        std::uint32_t cur; std::memcpy(&cur, p, sizeof(cur));
        if (cur != expected) return false;
        std::memcpy(p, &value, sizeof(value));
        return true;
    }
    MemoryWrite32(vaddr, value);
    return true;
}

bool UserCallbacksImpl::MemoryWriteExclusive64(VAddr vaddr, std::uint64_t value, std::uint64_t expected) {
    if (auto* p = ram_ptr(vaddr, 8)) {
        std::uint64_t cur; std::memcpy(&cur, p, sizeof(cur));
        if (cur != expected) return false;
        std::memcpy(p, &value, sizeof(value));
        return true;
    }
    MemoryWrite64(vaddr, value);
    return true;
}

std::uint8_t* UserCallbacksImpl::ram_ptr(VAddr vaddr, size_t size) const {
    size_t off = vaddr & ((1 << UserConfig::PAGE_BITS) - 1);
    if (off + size > (size_t(1) << UserConfig::PAGE_BITS)) return nullptr;
    uint8_t* page = (*instance->page_table)[vaddr >> UserConfig::PAGE_BITS];
    return page ? page + off : nullptr;
}

std::optional<std::uint32_t> UserCallbacksImpl::MemoryReadCode(VAddr vaddr) {
    size_t page_idx = vaddr >> UserConfig::PAGE_BITS;
    uint8_t* page = (*instance->page_table)[page_idx];
    if (page != nullptr) {
        size_t offset = vaddr & ((1 << UserConfig::PAGE_BITS) - 1);
        uint32_t word = 0;
        std::memcpy(&word, page + offset, sizeof(word));
        return word;
    }
    if (callbacks.read_32) {
        return callbacks.read_32(callbacks.user_data, vaddr);
    }
    return std::nullopt;
}

void UserCallbacksImpl::CallSVC(std::uint32_t swi) {
    instance->exit_reason = DYNARMIC_EXIT_REASON_SVC;
    if (callbacks.call_svc) {
        callbacks.call_svc(callbacks.user_data, swi);
    }
    instance->jit->HaltExecution();
}

void UserCallbacksImpl::ExceptionRaised(VAddr pc, Exception exception) {
    switch (exception) {
        case Exception::WaitForInterrupt:
            instance->exit_reason = DYNARMIC_EXIT_REASON_WFI;
            instance->jit->HaltExecution();
            break;
        case Exception::WaitForEvent:
            instance->exit_reason = DYNARMIC_EXIT_REASON_WFE;
            instance->jit->HaltExecution();
            break;
        case Exception::SendEvent:
        case Exception::SendEventLocal:
            instance->exit_reason = DYNARMIC_EXIT_REASON_SEV;
            break;
        case Exception::UndefinedInstruction: {
            uint32_t op = 0;
            auto c = MemoryReadCode(pc);
            if (c) op = *c;
            std::fprintf(stderr, "Dynarmic UndefinedInstruction at PC=0x%08x, opcode=0x%08x\n", pc, op);
            instance->exit_reason = DYNARMIC_EXIT_REASON_UNDEFINED;
            instance->jit->HaltExecution();
            break;
        }
        case Exception::NoExecuteFault:
            std::fprintf(stderr, "Dynarmic NoExecuteFault at PC=0x%08x\n", pc);
            instance->exit_reason = DYNARMIC_EXIT_REASON_NO_EXECUTE;
            instance->jit->HaltExecution();
            break;
        case Exception::Breakpoint:
            instance->exit_reason = DYNARMIC_EXIT_REASON_BREAKPOINT;
            instance->jit->HaltExecution();
            break;
        default:
            break;
    }
    if (callbacks.exception_raised) {
        callbacks.exception_raised(callbacks.user_data, pc, static_cast<uint32_t>(exception));
    }
}

extern "C" {

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks) {
    // One global exclusive monitor shared by all cores so LDREX/STREX on one
    // core invalidates reservations held by the others; each core gets its own id.
    static ExclusiveMonitor shared_monitor(4);
    static std::atomic<unsigned> next_id{0};
    const unsigned core_id = next_id.fetch_add(1) % 4;
    if (core_id == 0) {
        shared_monitor.Clear();
    }
    auto inst = std::make_unique<DynarmicInstance>();
    inst->callbacks = *callbacks;
    inst->page_table = std::make_unique<std::array<std::uint8_t*, UserConfig::NUM_PAGE_TABLE_ENTRIES>>();
    inst->page_table->fill(nullptr);

    inst->callbacks_impl = std::make_unique<UserCallbacksImpl>(inst->callbacks, inst.get());
    inst->cp15_impl = std::make_shared<CP15Impl>(inst.get());

    UserConfig conf;
    conf.callbacks = inst->callbacks_impl.get();
    conf.page_table = inst->page_table.get();
    conf.global_monitor = &shared_monitor;
    conf.arch_version = ArchVersion::v7;
    conf.define_unpredictable_behaviour = true;
    conf.processor_id = core_id;
    conf.absolute_offset_page_table = false;
    conf.enable_cycle_counting = true;
    conf.hook_hint_instructions = true;
    conf.hook_isb = false;
    conf.define_unpredictable_behaviour = true;

    conf.coprocessors[15] = inst->cp15_impl;
    conf.coprocessors[14] = inst->cp15_impl;

#if defined(__APPLE__)
    void* fastmem = mmap(nullptr, 1ULL << 32, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (fastmem != MAP_FAILED) {
        inst->fastmem_base = static_cast<uint8_t*>(fastmem);
        conf.fastmem_pointer = reinterpret_cast<uintptr_t>(inst->fastmem_base);
        conf.recompile_on_fastmem_failure = true;
        conf.fastmem_exclusive_access = false;
    }
#endif

    inst->jit = std::make_unique<Jit>(conf);
    return inst.release();
}

void dynarmic_destroy(DynarmicInstance* inst) {
    if (inst) {
#if defined(__APPLE__)
        if (inst->fastmem_base) {
            munmap(inst->fastmem_base, 1ULL << 32);
            inst->fastmem_base = nullptr;
        }
#endif
        delete inst;
    }
}

DynarmicExitReason dynarmic_run(DynarmicInstance* inst, uint64_t instructions, uint64_t* executed_instructions) {
    if (!inst) return DYNARMIC_EXIT_REASON_ERROR;

    inst->exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    inst->stop_requested.store(false, std::memory_order_relaxed);
    inst->callbacks_impl->ResetTicks(instructions);

    inst->jit->Run();

    if (inst->stop_requested.load(std::memory_order_relaxed)) {
        inst->exit_reason = DYNARMIC_EXIT_REASON_STOPPED;
    }

    if (executed_instructions) {
        *executed_instructions = inst->callbacks_impl->ExecutedTicks();
    }

    return inst->exit_reason;
}

DynarmicExitReason dynarmic_step(DynarmicInstance* inst) {
    if (!inst) return DYNARMIC_EXIT_REASON_ERROR;
    inst->exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    inst->callbacks_impl->ResetTicks(1);
    inst->jit->Step();
    return inst->exit_reason;
}

void dynarmic_stop(DynarmicInstance* inst) {
    if (inst) {
        inst->stop_requested.store(true, std::memory_order_relaxed);
        inst->jit->HaltExecution();
    }
}

void dynarmic_clear_cache(DynarmicInstance* inst) {
    if (inst) {
        inst->jit->ClearCache();
    }
}

void dynarmic_invalidate_cache(DynarmicInstance* inst, uint32_t vaddr, size_t length) {
    if (inst) {
        inst->jit->InvalidateCacheRange(vaddr, length);
    }
}

uint32_t dynarmic_get_reg(DynarmicInstance* inst, uint32_t reg) {
    if (!inst || reg > 15) return 0;
    return inst->jit->Regs()[reg];
}

void dynarmic_set_reg(DynarmicInstance* inst, uint32_t reg, uint32_t val) {
    if (inst && reg <= 15) {
        inst->jit->Regs()[reg] = val;
    }
}

uint32_t dynarmic_get_cpsr(DynarmicInstance* inst) {
    return inst ? inst->jit->Cpsr() : 0;
}

void dynarmic_set_cpsr(DynarmicInstance* inst, uint32_t val) {
    if (inst) {
        inst->jit->SetCpsr(val);
    }
}

uint32_t dynarmic_get_spsr(DynarmicInstance* inst) {
    return inst ? inst->jit->Spsr() : 0;
}

void dynarmic_set_spsr(DynarmicInstance* inst, uint32_t val) {
    if (inst) {
        inst->jit->SetSpsr(val);
    }
}

uint32_t dynarmic_get_fpscr(DynarmicInstance* inst) {
    return inst ? inst->jit->Fpscr() : 0;
}

void dynarmic_set_fpscr(DynarmicInstance* inst, uint32_t val) {
    if (inst) {
        inst->jit->SetFpscr(val);
    }
}

void dynarmic_map_pages(DynarmicInstance* inst, uint32_t start_page, uint32_t num_pages, uint8_t* host_ptr) {
    if (!inst || num_pages == 0) return;
    uint32_t end_page = start_page + num_pages;
    if (end_page > UserConfig::NUM_PAGE_TABLE_ENTRIES) {
        end_page = UserConfig::NUM_PAGE_TABLE_ENTRIES;
    }
    for (uint32_t p = start_page; p < end_page; ++p) {
        (*inst->page_table)[p] = host_ptr + (static_cast<size_t>(p - start_page) << 12);
    }
#if defined(__APPLE__)
    if (inst->fastmem_base && host_ptr && (reinterpret_cast<uintptr_t>(host_ptr) & 0xfff) == 0) {
        size_t len = static_cast<size_t>(end_page - start_page) << 12;
        mach_vm_address_t target = reinterpret_cast<mach_vm_address_t>(
            inst->fastmem_base + (static_cast<uint64_t>(start_page) << 12));
        vm_prot_t cur_prot = 0, max_prot = 0;
        kern_return_t kr = mach_vm_remap(
            mach_task_self(),
            &target,
            len,
            0,
            VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
            mach_task_self(),
            reinterpret_cast<mach_vm_address_t>(host_ptr),
            FALSE,
            &cur_prot,
            &max_prot,
            VM_INHERIT_NONE
        );
        if (kr != KERN_SUCCESS) {
            mprotect(reinterpret_cast<void*>(target), len, PROT_NONE);
        }
    }
#endif
}

void dynarmic_unmap_pages(DynarmicInstance* inst, uint32_t start_page, uint32_t num_pages) {
    if (!inst || num_pages == 0) return;
    uint32_t end_page = start_page + num_pages;
    if (end_page > UserConfig::NUM_PAGE_TABLE_ENTRIES) {
        end_page = UserConfig::NUM_PAGE_TABLE_ENTRIES;
    }
    for (uint32_t p = start_page; p < end_page; ++p) {
        (*inst->page_table)[p] = nullptr;
    }
#if defined(__APPLE__)
    if (inst->fastmem_base) {
        size_t len = static_cast<size_t>(end_page - start_page) << 12;
        void* target = inst->fastmem_base + (static_cast<uint64_t>(start_page) << 12);
        mprotect(target, len, PROT_NONE);
    }
#endif
}

void dynarmic_map_page(DynarmicInstance* inst, uint32_t page_index, uint8_t* host_ptr) {
    if (inst && page_index < UserConfig::NUM_PAGE_TABLE_ENTRIES) {
        (*inst->page_table)[page_index] = host_ptr;
#if defined(__APPLE__)
        if (inst->fastmem_base && host_ptr && (reinterpret_cast<uintptr_t>(host_ptr) & 0xfff) == 0) {
            mach_vm_address_t target = reinterpret_cast<mach_vm_address_t>(
                inst->fastmem_base + (static_cast<uint64_t>(page_index) << 12));
            vm_prot_t cur_prot = 0, max_prot = 0;
            kern_return_t kr = mach_vm_remap(
                mach_task_self(),
                &target,
                4096,
                0,
                VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
                mach_task_self(),
                reinterpret_cast<mach_vm_address_t>(host_ptr),
                FALSE,
                &cur_prot,
                &max_prot,
                VM_INHERIT_NONE
            );
            if (kr != KERN_SUCCESS) {
                mprotect(reinterpret_cast<void*>(target), 4096, PROT_NONE);
            }
        }
#endif
    }
}

void dynarmic_unmap_page(DynarmicInstance* inst, uint32_t page_index) {
    if (inst && page_index < UserConfig::NUM_PAGE_TABLE_ENTRIES) {
        (*inst->page_table)[page_index] = nullptr;
#if defined(__APPLE__)
        if (inst->fastmem_base) {
            void* target = inst->fastmem_base + (static_cast<uint64_t>(page_index) << 12);
            mprotect(target, 4096, PROT_NONE);
        }
#endif
    }
}

void dynarmic_set_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2, uint32_t val) {
    if (inst && inst->cp15_impl) {
        inst->cp15_impl->Write(opc1, crn, crm, opc2, val);
    }
}

uint32_t dynarmic_get_cp15(DynarmicInstance* inst, uint32_t opc1, uint32_t crn, uint32_t crm, uint32_t opc2) {
    if (inst && inst->cp15_impl) {
        return inst->cp15_impl->Read(opc1, crn, crm, opc2);
    }
    return 0;
}

void dynarmic_set_cache_ids(DynarmicInstance* inst, uint32_t l1d, uint32_t l1i, uint32_t l2) {
    if (inst && inst->cp15_impl) {
        inst->cp15_impl->SetCacheIds(l1d, l1i, l2);
    }
}

}
