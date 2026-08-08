//! The interface between the CPU and everything else.

/// The CPU's view of the outside world.
///
/// Four methods rather than two, because the Z80 has a **separate 16-bit I/O
/// address space** and the vectors verify it as a distinct pin state (`r--i` and
/// `-w-i`, against `r-m-` and `-wm-` for memory). A core that routed ports
/// through `read`/`write` would pass every register comparison and fail every
/// bus-trace comparison on the I/O pages.
///
/// There is deliberately no `read16`: the Z80's data bus is 8 bits, and every
/// 16-bit access is two byte accesses in a defined order. Composing them in the
/// core is what makes the T-state counts fall out instead of needing a table —
/// the same reasoning `m68k::Bus` records for its missing `read32`.
pub trait Bus {
    /// Reads one byte of memory.
    fn read(&mut self, addr: u16) -> u8;

    /// Writes one byte of memory.
    fn write(&mut self, addr: u16, val: u8);

    /// Reads one byte from an I/O port.
    ///
    /// `port` is the **full 16 bits**, not the low 8.
    ///
    /// `IN A,(n)` puts `A` on the high half and `IN r,(C)` puts `B` there, so a
    /// core that masked to 8 bits would pass the common cases and fail those two.
    /// The suite's `ports` array records 16 bits for exactly this reason.
    fn port_in(&mut self, port: u16) -> u8;

    /// Writes one byte to an I/O port. `port` is the full 16 bits, as for
    /// [`Bus::port_in`].
    fn port_out(&mut self, port: u16, val: u8);
}
