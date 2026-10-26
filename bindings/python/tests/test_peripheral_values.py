"""Public enum and value conversions, including production builds without USB."""

from peripheral_contract import check_peripheral_values


def test_peripheral_values() -> None:
    assert check_peripheral_values()
