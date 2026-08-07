//! The interface between the CPU and everything else.

/// The CPU's view of the outside world.
///
/// The 68000 has a 16-bit data bus, so there is deliberately no `read32`:
/// long accesses are composed from two 16-bit accesses by the core, which is
/// both what the hardware does and what makes the cycle counts fall out
/// correctly.
///
/// Implementors own all memory, peripherals, and timing. Addresses are already
/// masked to 24 bits by the core.
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn write8(&mut self, addr: u32, val: u8);
    fn write16(&mut self, addr: u32, val: u16);
}
