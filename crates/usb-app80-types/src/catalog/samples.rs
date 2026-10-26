//! Direct adjustment sample requests. Returned bytes remain raw snapshots;
//! none of these commands composes a capture or bulk-transfer workflow.

/// One control request used by the adjustment sample commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentControlAction {
    Read { address: u16 },
    Write { address: u16, value: u8 },
}

impl AdjustmentControlAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        let (operation, address, value) = match self {
            Self::Read { address } => (0, address, u8::MAX),
            Self::Write { address, value } => (1, address, value),
        };
        let [high, low] = address.to_be_bytes();
        put(0, operation);
        put(1, high);
        put(2, low);
        put(3, value);
    }
}

#[cfg(test)]
mod tests {
    use super::AdjustmentControlAction;

    fn logical(action: AdjustmentControlAction) -> [u8; 4] {
        let mut bytes = [0; 4];
        action.encode(&mut |index, value| bytes[index] = value);
        bytes
    }

    #[test]
    fn control_requests_encode_operation_address_and_read_sentinel() {
        assert_eq!(
            logical(AdjustmentControlAction::Read { address: 0x1234 }),
            [0, 0x12, 0x34, u8::MAX]
        );
        assert_eq!(
            logical(AdjustmentControlAction::Write {
                address: 0xabcd,
                value: 0x56,
            }),
            [1, 0xab, 0xcd, 0x56]
        );
    }
}
