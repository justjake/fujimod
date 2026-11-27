#include "dynarmic_ffi.h"
#include "dynarmic/interface/A32/a32.h"
#include "dynarmic/interface/A32/config.h"
#include "dynarmic/interface/A32/coprocessor.h"
#include "dynarmic/interface/exclusive_monitor.h"
#include <memory>
#include <array>
#include <cstring>
#include <atomic>

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

    std::optional<std::uint32_t> MemoryReadCode(VAddr vaddr) override;

    std::uint8_t MemoryRead8(VAddr address) override {
        return static_cast<std::uint8_t>(callbacks.read(callbacks.user_data, address, 1));
    }

    void MemoryWrite8(VAddr address, std::uint8_t value) override {
        callbacks.write(callbacks.user_data, address, 1, value);
    }

    std::uint16_t MemoryRead16(VAddr address) override {
        return static_cast<std::uint16_t>(callbacks.read(callbacks.user_data, address, 2));
    }

    void MemoryWrite16(VAddr address, std::uint16_t value) override {
        callbacks.write(callbacks.user_data, address, 2, value);
    }

    std::uint32_t MemoryRead32(VAddr address) override {
        return static_cast<std::uint32_t>(callbacks.read(callbacks.user_data, address, 4));
    }

    void MemoryWrite32(VAddr address, std::uint32_t value) override {
        callbacks.write(callbacks.user_data, address, 4, value);
    }

    std::uint64_t MemoryRead64(VAddr address) override {
        return static_cast<std::uint64_t>(callbacks.read(callbacks.user_data, address, 8));
    }

    void MemoryWrite64(VAddr address, std::uint64_t value) override {
        callbacks.write(callbacks.user_data, address, 8, value);
    }

    void InterpreterFallback(VAddr, size_t) override;

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
    std::unique_ptr<UserCallbacksImpl> callbacks_impl;
    std::shared_ptr<CP15Impl> cp15_impl;
    ExclusiveMonitor monitor{1};
    std::unique_ptr<Jit> jit;
    DynarmicExitReason exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    std::atomic<bool> stop_requested{false};
};

std::optional<std::uint32_t> UserCallbacksImpl::MemoryReadCode(VAddr address) {
    std::uint32_t word;
    if (callbacks.read_code(callbacks.user_data, address, &word)) {
        return word;
    }
    return std::nullopt;
}

void UserCallbacksImpl::InterpreterFallback(VAddr, size_t) {
    instance->exit_reason = DYNARMIC_EXIT_REASON_UNDEFINED;
    instance->jit->HaltExecution();
}

void UserCallbacksImpl::CallSVC(std::uint32_t) {
    instance->exit_reason = DYNARMIC_EXIT_REASON_SVC;
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
            instance->jit->HaltExecution();
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
}

extern "C" {

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks) {
    auto inst = std::make_unique<DynarmicInstance>();
    inst->callbacks = *callbacks;

    inst->callbacks_impl = std::make_unique<UserCallbacksImpl>(inst->callbacks, inst.get());
    inst->cp15_impl = std::make_shared<CP15Impl>(inst.get());

    UserConfig conf;
    conf.callbacks = inst->callbacks_impl.get();
    conf.global_monitor = &inst->monitor;
    conf.arch_version = ArchVersion::v7;
    conf.check_halt_on_memory_access = true;
    conf.enable_cycle_counting = true;
    conf.hook_hint_instructions = true;
    conf.hook_isb = false;

    conf.coprocessors[15] = inst->cp15_impl;

    inst->jit = std::make_unique<Jit>(conf);
    return inst.release();
}

void dynarmic_destroy(DynarmicInstance* inst) {
    if (inst) {
        delete inst;
    }
}

DynarmicExitReason dynarmic_step(DynarmicInstance* inst, uint64_t* executed) {
    if (!inst) return DYNARMIC_EXIT_REASON_ERROR;
    inst->exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    inst->callbacks_impl->ResetTicks(1);
    inst->stop_requested.store(false, std::memory_order_relaxed);
    inst->jit->ClearHalt(Dynarmic::HaltReason::UserDefined1 | Dynarmic::HaltReason::MemoryAbort);
    inst->jit->Step();
    *executed = inst->callbacks_impl->ExecutedTicks();
    if (inst->stop_requested.load(std::memory_order_relaxed)) {
        return DYNARMIC_EXIT_REASON_STOPPED;
    }
    return inst->exit_reason;
}

void dynarmic_abort_access(DynarmicInstance* inst) {
    inst->jit->HaltExecution(Dynarmic::HaltReason::MemoryAbort);
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

uint32_t dynarmic_get_fpscr(DynarmicInstance* inst) {
    return inst ? inst->jit->Fpscr() : 0;
}

void dynarmic_set_fpscr(DynarmicInstance* inst, uint32_t val) {
    if (inst) {
        inst->jit->SetFpscr(val);
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
