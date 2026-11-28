#include "dynarmic_ffi.h"
#include "dynarmic/interface/A32/a32.h"
#include "dynarmic/interface/A32/config.h"
#include "dynarmic/interface/A32/coprocessor.h"
#include "dynarmic/interface/exclusive_monitor.h"
#include "dynarmic/frontend/A32/a32_ir_emitter.h"
#include "dynarmic/ir/basic_block.h"
#include "dynarmic/ir/cond.h"
#include <memory>
#include <array>
#include <cstring>
#include <atomic>
#include <bit>
#include <utility>

using Dynarmic::ExclusiveMonitor;
using namespace Dynarmic::A32;

struct DynarmicInstance;

class CP15Impl final : public Coprocessor {
public:
    explicit CP15Impl(DynarmicInstance* inst, bool rprofile) : instance(inst) {
        regs.fill(0);
        if (rprofile) {
            regs[encode(0, 0, 0, 0)] = 0x411fc153; // MIDR (ARM Cortex-R5 r1p3)
            return;
        }
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

    static void GuardTransfer(std::uint64_t ctx, std::uint64_t word, std::uint64_t base) {
        const auto* self = reinterpret_cast<UserCallbacksImpl*>(ctx);
        self->callbacks.guard_transfer(self->callbacks.user_data, word, base);
    }

    bool PreCodeReadHook(bool thumb, VAddr pc, IREmitter& ir) override;

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

    std::uint32_t MemoryRead32Timed(VAddr address, std::uint64_t ticks_remaining) override {
        if (ticks_remaining > remaining_ticks)
            return MemoryRead32(address);
        return callbacks.timed_read32(callbacks.user_data, address, remaining_ticks - ticks_remaining);
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
    struct PrefetchedCode {
        VAddr address;
        std::uint32_t word;
    };

    std::optional<std::uint32_t> ReadCheckedCode(VAddr address);
#if defined(__aarch64__)
    std::optional<Dynarmic::IR::A32RestoringDivision> MatchRestoringDivision(VAddr pc, uint32_t first);
#endif
    std::optional<PrefetchedCode> prefetched_code;
    bool compiled_multiple_transfer = false;
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
    std::array<std::uint8_t*, UserConfig::NUM_PAGE_TABLE_ENTRIES> page_table{};
    std::unique_ptr<Jit> jit;
    DynarmicExitReason exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    std::atomic<bool> stop_requested{false};
    bool vfp_enabled = false;
};

extern "C" void fujimod_dynarmic_guard_transfer(std::uint64_t ctx, std::uint64_t word, std::uint64_t base) {
    UserCallbacksImpl::GuardTransfer(ctx, word, base);
}

constexpr uint32_t exit_before_instruction = 0x01000000;
constexpr uint32_t max_block_instructions = 24;

#if defined(__aarch64__)
std::optional<Dynarmic::IR::A32RestoringDivision> UserCallbacksImpl::MatchRestoringDivision(VAddr pc, uint32_t first) {
    using Dynarmic::IR::A32RestoringDivision;
    constexpr uint32_t bytes = A32RestoringDivision::instruction_count * 4;
    if ((first & 0xfff0fff0) != 0xe1500000 || pc > UINT32_MAX - bytes) {
        return std::nullopt;
    }
    const uint32_t remainder = (first >> 16) & 15;
    const uint32_t divisor = first & 15;
    const uint32_t remainder_operand = remainder << 16;
    const uint32_t compare = 0xe1500000 | remainder_operand | divisor;
    const uint32_t subtract = 0x20400000 | remainder_operand | (remainder << 12) | divisor;
    if (ReadCheckedCode(pc + 4) != subtract) {
        return std::nullopt;
    }
    const auto accumulate = ReadCheckedCode(pc + 8);
    if (!accumulate) {
        return std::nullopt;
    }
    const uint32_t quotient = (*accumulate >> 12) & 15;
    const uint32_t place = *accumulate & 15;
    const uint32_t registers = (1u << remainder) | (1u << divisor) | (1u << quotient) | (1u << place);
    const uint32_t or_result = 0x21800000 | (quotient << 16) | (quotient << 12) | place;
    if (std::popcount(registers) != 4 || (registers & (1u << 15)) != 0 || *accumulate != or_result) {
        return std::nullopt;
    }
    for (uint32_t shift = 1; shift < 4; ++shift) {
        const uint32_t lsr = (shift << 7) | (1 << 5);
        const uint32_t group = pc + shift * 12;
        if (ReadCheckedCode(group) != (compare | lsr)
            || ReadCheckedCode(group + 4) != (subtract | lsr)
            || ReadCheckedCode(group + 8) != (or_result | lsr)) {
            return std::nullopt;
        }
    }
    const auto branch = ReadCheckedCode(pc + bytes - 4);
    // BNE back to the first comparison, relative to the architectural PC.
    constexpr uint32_t branch_offset = (0u - (bytes + 4) / 4) & 0x00ffffff;
    if (ReadCheckedCode(pc + 48) != (0xe3500000 | remainder_operand)
        || ReadCheckedCode(pc + 52) != (0x11b00220 | (place << 12) | place)
        || ReadCheckedCode(pc + 56) != (0x11a00220 | (divisor << 12) | divisor)
        || branch != (0x1a000000 | branch_offset)) {
        return std::nullopt;
    }
    return A32RestoringDivision{static_cast<uint8_t>(remainder), static_cast<uint8_t>(divisor),
                                static_cast<uint8_t>(place), static_cast<uint8_t>(quotient)};
}
#endif

bool UserCallbacksImpl::PreCodeReadHook(bool thumb, VAddr pc, IREmitter& ir) {
    const LocationDescriptor start{ir.block.Location()};
    if (start.SingleStepping()) return true;
    const bool entry = start.PC() == pc;
    prefetched_code.reset();
    if (entry) compiled_multiple_transfer = false;
    const auto word = thumb ? std::nullopt : ReadCheckedCode(pc);
#if defined(__aarch64__)
    if (entry && word) {
        if (auto division = MatchRestoringDivision(pc, *word)) {
            ir.block.SetRestoringDivision(*division);
        }
    }
#endif
    const auto consume_word = [&] {
        if (word) {
            prefetched_code = PrefetchedCode{pc, *word};
        }
        return true;
    };
    uint32_t kind = word ? callbacks.batch_instruction(*word) : 0;
    if (kind >= 4 && kind <= 6 && (instance->jit->Cpsr() & 31) == 0x10) kind = 0;
    if (kind == 8) kind = instance->vfp_enabled ? 1 : 0;
    if (kind == 9 || kind == 10) {
        // This affects dispatch only. The runtime proof checks the current
        // instruction bytes again before advancing the loop.
        const uint32_t span = kind == 9 ? 12 : 8;
        kind = 1;
        if (pc >= span) {
            const auto first = ReadCheckedCode(pc - span);
            const auto second = ReadCheckedCode(pc - span + 4);
            const auto third = span == 12 ? ReadCheckedCode(pc - 4) : std::optional<std::uint32_t>{0};
            if (first && second && third && callbacks.is_counted_loop(*word, *first, *second, *third)) {
                kind = 7;
            }
        }
    }
    if (!entry) {
        if (pc - start.PC() < max_block_instructions * 4) {
            if (kind == 1) return consume_word();
#if defined(__aarch64__)
            constexpr uint32_t condition_and_writeback = 0xf0200000;
            constexpr uint32_t unconditional_without_explicit_writeback = 0xe0000000;
            // The replay path can recover a post-indexed base directly
            // from the faulting address, even after a retired block prefix.
            if (kind == 2 && !compiled_multiple_transfer
                && (*word & condition_and_writeback) == unconditional_without_explicit_writeback
                && ((*word >> 12) & 15) != 15) {
                return consume_word();
            }
#endif
        }
        ir.SetTerm(Dynarmic::IR::Term::LinkBlock{ir.current_location});
        return false;
    }
    if (kind == 4 || kind == 5) {
        if (kind == 5) {
            ir.SetCpsr(ir.Or(ir.GetCpsr(), ir.Imm32(0x80)));
        }
        ir.current_location = ir.current_location.AdvancePC(4);
        ir.block.CycleCount()++;
        ir.SetTerm(Dynarmic::IR::Term::LinkBlock{ir.current_location});
        return false;
    }
    if (kind == 3) {
        const auto reg = static_cast<Reg>((*word >> 16) & 15);
        ir.CallHostFunction(&fujimod_dynarmic_guard_transfer,
            ir.Imm64(reinterpret_cast<uintptr_t>(this)), ir.Imm64(*word),
            ir.ZeroExtendWordToLong(ir.GetRegister(reg)));
    }
    if (kind == 7) {
        // Fall-through retires the branch in the JIT. A taken back edge
        // exits with no ticks so the caller can try to accelerate the loop.
        ir.current_location = ir.current_location.AdvancePC(4);
        ir.block.SetCondition(static_cast<Dynarmic::IR::Cond>(*word >> 28));
        ir.block.SetConditionFailedLocation(ir.current_location);
        ir.block.ConditionFailedCycleCount() = 1;
    } else if (kind != 0) {
        compiled_multiple_transfer = kind == 3;
        return consume_word();
    }
    ir.BranchWritePC(ir.Imm32(pc));
    ir.CallSupervisor(ir.Imm32(exit_before_instruction));
    ir.SetTerm(Dynarmic::IR::Term::CheckHalt{Dynarmic::IR::Term::ReturnToDispatch{}});
    return false;
}

std::optional<std::uint32_t> UserCallbacksImpl::MemoryReadCode(VAddr address) {
    if (auto prefetched = std::exchange(prefetched_code, std::nullopt);
        prefetched && prefetched->address == address) {
        return prefetched->word;
    }
    return ReadCheckedCode(address);
}

std::optional<std::uint32_t> UserCallbacksImpl::ReadCheckedCode(VAddr address) {
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

void UserCallbacksImpl::CallSVC(std::uint32_t immediate) {
    instance->exit_reason = immediate == exit_before_instruction
        ? DYNARMIC_EXIT_REASON_REPLAY : DYNARMIC_EXIT_REASON_SVC;
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

DynarmicInstance* dynarmic_create(const DynarmicCallbacks* callbacks, bool rprofile) {
    auto inst = std::make_unique<DynarmicInstance>();
    inst->callbacks = *callbacks;

    inst->callbacks_impl = std::make_unique<UserCallbacksImpl>(inst->callbacks, inst.get());
    inst->cp15_impl = std::make_shared<CP15Impl>(inst.get(), rprofile);

    UserConfig conf;
    conf.callbacks = inst->callbacks_impl.get();
    conf.global_monitor = &inst->monitor;
    conf.arch_version = ArchVersion::v7;
    // ARM64's inline page table callback path checks MemoryAbort.
    conf.page_table = &inst->page_table;
    conf.absolute_offset_page_table = true;
    conf.timed_read32_cache = static_cast<Dynarmic::A32::TimedRead32Cache*>(callbacks->inline_read32);
    conf.page_table_pointer_mask_bits = 1;
    conf.detect_misaligned_access_via_page_table = 16 | 32 | 64;
    conf.only_detect_misalignment_via_page_table_on_page_boundary = true;
    conf.check_halt_on_memory_access = true;
    conf.optimizations = conf.optimizations & ~Dynarmic::OptimizationFlag::ReturnStackBuffer;
#if !defined(__aarch64__)
    conf.optimizations = conf.optimizations & ~Dynarmic::OptimizationFlag::FastDispatch;
#endif
    conf.enable_cycle_counting = true;
    conf.check_cycle_budget = true;
    conf.guard_native_writes = true;
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

void dynarmic_set_translation_context(DynarmicInstance* inst, uint32_t value, bool vfp_enabled) {
    inst->vfp_enabled = vfp_enabled;
#if defined(__aarch64__)
    inst->jit->SetTranslationContext(value);
#endif
}

void dynarmic_observe_stores(DynarmicInstance* inst, bool enabled) {
#if defined(__aarch64__)
    inst->jit->ObserveStores(enabled);
#endif
}

void dynarmic_read_gprs(DynarmicInstance* inst, uint32_t* values) {
    const auto& registers = inst->jit->Regs();
    std::memcpy(values, registers.data(), 15 * sizeof(uint32_t));
}

void dynarmic_write_loop_state(DynarmicInstance* inst, const uint32_t* values, uint32_t cpsr, bool flags_changed) {
    auto& registers = inst->jit->Regs();
    std::memcpy(registers.data(), values, 15 * sizeof(uint32_t));
    if (flags_changed) {
        inst->jit->SetCpsr(cpsr);
    }
}

void dynarmic_read_snapshot(DynarmicInstance* inst, uint32_t* ext_regs, uint32_t* cp15, uint32_t* cache_ids) {
    const auto& vectors = inst->jit->ExtRegs();
    const auto& system = *inst->cp15_impl;
    std::memcpy(ext_regs, vectors.data(), vectors.size() * sizeof(uint32_t));
    std::memcpy(cp15, system.regs.data(), system.regs.size() * sizeof(uint32_t));
    std::memcpy(cache_ids, system.cache_ids.data(), system.cache_ids.size() * sizeof(uint32_t));
}

void dynarmic_write_snapshot(DynarmicInstance* inst, const uint32_t* ext_regs, const uint32_t* cp15, const uint32_t* cache_ids) {
    auto& vectors = inst->jit->ExtRegs();
    auto& system = *inst->cp15_impl;
    std::memcpy(vectors.data(), ext_regs, vectors.size() * sizeof(uint32_t));
    std::memcpy(system.regs.data(), cp15, system.regs.size() * sizeof(uint32_t));
    std::memcpy(system.cache_ids.data(), cache_ids, system.cache_ids.size() * sizeof(uint32_t));
    inst->jit->ClearExclusiveState();
}

uint8_t** dynarmic_data_pages(DynarmicInstance* inst) {
    return inst->page_table.data();
}

DynarmicExitReason dynarmic_run(DynarmicInstance* inst, uint64_t budget, uint64_t* executed) {
    inst->exit_reason = DYNARMIC_EXIT_REASON_TIMEOUT;
    inst->callbacks_impl->ResetTicks(budget);
    inst->stop_requested.store(false, std::memory_order_relaxed);
    inst->jit->ClearHalt(Dynarmic::HaltReason::UserDefined1 | Dynarmic::HaltReason::MemoryAbort);
    inst->jit->Run();
    *executed = inst->callbacks_impl->ExecutedTicks();
    if (inst->stop_requested.load(std::memory_order_relaxed)) {
        return DYNARMIC_EXIT_REASON_STOPPED;
    }
    return inst->exit_reason;
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

void dynarmic_memory_control(DynarmicInstance* inst, DynarmicMemoryControl* control) {
    const auto& cp = *inst->cp15_impl;
    control->sctlr = cp.Read(0, 1, 0, 0); // SCTLR
    control->ttbr0 = cp.Read(0, 2, 0, 0); // TTBR0
    control->ttbr1 = cp.Read(0, 2, 0, 1); // TTBR1
    control->ttbcr = cp.Read(0, 2, 0, 2); // TTBCR
    control->dacr = cp.Read(0, 3, 0, 0); // DACR
    control->fcseidr = cp.Read(0, 13, 0, 0); // FCSEIDR
    control->cpsr = inst->jit->Cpsr();
    control->cpacr = cp.Read(0, 1, 0, 2); // CPACR
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
