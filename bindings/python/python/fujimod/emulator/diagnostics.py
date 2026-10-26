"""Register identifiers for isolated driver diagnostics."""

from . import registers

SCALAR_REGISTERS = {
    "cpacr": registers.CPACR,
    "fpscr": registers.FPSCR,
    "fpexc": registers.FPEXC,
    "esr": registers.ESR,
}
