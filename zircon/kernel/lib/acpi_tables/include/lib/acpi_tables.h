// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef KERNEL_LIB_ACPI_TABLES_H
#define KERNEL_LIB_ACPI_TABLES_H

#include <lib/acpi_lite/structures.h>
#include <zircon/types.h>

#include <arch/x86/apic.h>
#include <fbl/function.h>

namespace acpi_lite {
class AcpiParser;
}

// TODO(edcoyne): rename this to C++ naming.
struct acpi_hpet_descriptor {
  uint64_t address;
  bool port_io;

  uint16_t minimum_tick;
  uint8_t sequence;
};

constexpr uint8_t kAcpiMaxNumaRegions = 5;

struct AcpiNumaRegion {
  uint64_t base_address;
  uint64_t length;
};

struct AcpiNumaDomain {
  uint32_t domain = 0xFF;
  AcpiNumaRegion memory[kAcpiMaxNumaRegions];
  uint8_t memory_count = 0;
};

// ACPI constants.
inline constexpr uint8_t kAcpiAddressSpaceMemory = 0;  // Memory/MMIO address.
inline constexpr uint8_t kAcpiAddressSpaceIOPort = 1;  // I/O port address.

// Describes a dedicated system debug port suitable for low-level
// debugging and diagnostics.
//
// Currently, we only support a 16550-compatible UART using MMIO.
struct AcpiDebugPortDescriptor {
  // Physical address of the 16550 MMIO registers.
  paddr_t address;
};

// Provide access to ACPI tables.
//
// Wraps acpi_lite functions to allow testing.
class AcpiTableProvider {
 public:
  virtual ~AcpiTableProvider() = default;

  // Looks up table, on success sets header to point to table. Maintains
  // ownership of the table's memory.
  virtual zx_status_t GetTable(AcpiSignature signature, char** header) const = 0;
};

// Implementation of AcpiTableProvider that is backed by the acpi_lite library.
class AcpiLiteTableProvider final : public AcpiTableProvider {
 public:
  explicit AcpiLiteTableProvider(const acpi_lite::AcpiParser* parser) : parser_(parser) {}

  // |AcpiTableProvider| implementation.
  zx_status_t GetTable(AcpiSignature signature, char** header) const final;

 private:
  const acpi_lite::AcpiParser* parser_;  // Owned elsewhere.
};

// Designed to read and parse APIC tables, other functions of the APIC
// subsystem are out of scope of this class. This class can work before dynamic memory
// allocation is available.
class AcpiTables {
 public:
  explicit AcpiTables(const AcpiTableProvider* tables) : tables_(tables) {}

  // Sets count equal to the number of cpus in the system.
  zx_status_t cpu_count(uint32_t* count) const;

  // Populates the apic_ids array with the apic ids of all cpus in the system.
  // Sets apic_id_count equal to the number of ids written to the array and is
  // bounded by array_size.
  zx_status_t cpu_apic_ids(uint32_t* apic_ids, uint32_t array_size, uint32_t* apic_id_count) const;

  // Sets count equal to the number of IO APICs in the system.
  zx_status_t io_apic_count(uint32_t* count) const;

  // Populates the io_apics array with data about the IO APICS in the system,
  // bounded by array_size. io_apics_count will contain how many io_apics were
  // populated in the array.
  zx_status_t io_apics(io_apic_descriptor* io_apics, uint32_t array_size,
                       uint32_t* io_apics_count) const;

  // Populates overrides with data on all overrides, bounded by array_size.
  // override_count will contain the number of overrides populated in the
  // array.
  zx_status_t interrupt_source_overrides(io_apic_isa_override* overrides, uint32_t array_size,
                                         uint32_t* override_count) const;

  // Sets count equal to the number of overrides registered in the system.
  zx_status_t interrupt_source_overrides_count(uint32_t* count) const;

  // Lookup high precision event timer information. Returns ZX_OK and
  // populates hpet if successful, otherwise returns error.
  zx_status_t hpet(acpi_hpet_descriptor* hpet) const;

  // Lookup low-level debug port information. Returns ZX_OK and
  // populates info if successful, otherwise returns error.
  zx_status_t debug_port(AcpiDebugPortDescriptor* desc) const;

  // Vists all pairs of cpu apic id and NumaRegion.
  // Visitor is expected to have the signature:
  // void(const AcpiNumaRegion&, uint32_t)
  zx_status_t VisitCpuNumaPairs(fbl::Function<void(const AcpiNumaDomain&, uint32_t)> visitor) const;

  // Set / get a default instance of AcpiTables.
  //
  // Caller is responsible for synchronising SetDefault(), typically by
  // calling this during system startup.
  //
  // These default instances allow existing production code to use an
  // AcpiTable instance without having to plumb it everywhere.
  static void SetDefault(const AcpiTables* table);
  static const AcpiTables& Default();

  // Initialise a default instance using the acpi_lite library.

 private:
  zx_status_t NumInMadt(uint8_t type, uint32_t* count) const;

  // For each subtable of type run visitor.
  // We can't take a std::function for the visitor because that can use dynamic
  // memory.
  template <typename V>
  zx_status_t ForEachInMadt(uint8_t type, V visitor) const;

  // Set start and end to the first and last (respectively) records in the
  // MADT table.
  zx_status_t GetMadtRecordLimits(uintptr_t* start, uintptr_t* end) const;

  const AcpiTableProvider* const tables_;

  inline static const AcpiTables* default_ = nullptr;
};

#endif  // KERNEL_LIB_APIC_TABLES_H
