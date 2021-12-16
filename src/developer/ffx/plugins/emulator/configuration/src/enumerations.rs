// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module holds the common data types for emulator engines. These are implementation-agnostic
//! data types, not the engine-specific command types that each engine will define for itself. These
//! types will be directly deserializable from the PBM, and converted into engine-specific types at
//! runtime.

use sdk_metadata::{display_impl, TargetArchitecture};
use serde::{Deserialize, Serialize};

/// Selector for which type of hardware acceleration will be enabled for the emulator.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccelerationMode {
    /// The emulator will set the acceleration mode according to the host system's capabilities.
    Auto,

    /// KVM or similar acceleration will be enabled.
    Hyper,

    /// Hardware acceleration is disabled.
    None,
}

impl Default for AccelerationMode {
    fn default() -> Self {
        AccelerationMode::None
    }
}

display_impl!(AccelerationMode);

/// Selector for the launcher's output once the system is running.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleType {
    /// The launcher will finish by opening the Fuchsia serial console.
    Console,

    /// The launcher will finish by opening the Qemu menu terminal.
    Monitor,

    /// The launcher will finish by returning the user to the host's command prompt.
    None,
}

impl Default for ConsoleType {
    fn default() -> Self {
        ConsoleType::None
    }
}

display_impl!(ConsoleType);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    /// Fuchsia Emulator based on AEMU. Supports graphics.
    Femu,

    /// Qemu emulator. Version 5.
    Qemu,
}

impl Default for EngineType {
    fn default() -> Self {
        EngineType::Femu
    }
}

display_impl!(EngineType);

/// Selector for which type of graphics acceleration to enable for the emulator.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuType {
    /// Let the emulator choose between hardware or software graphics
    /// acceleration based on your computer setup.
    Auto,

    /// Use the GPU on your computer for hardware acceleration. This option
    /// typically provides the highest graphics quality and performance for the
    /// emulator. However, if your graphics drivers have issues rendering
    /// OpenGL, you might need to use the swiftshader_indirect or
    /// angle_indirect options.
    Host,

    /// Use a Quick Boot-compatible variant of SwiftShader to render graphics
    /// using software acceleration. This option is a good alternative to host
    /// mode if your computer can't use hardware acceleration.
    SwiftshaderIndirect,

    /// Use guest-side software rendering. This option provides the lowest
    /// graphics quality and performance for the emulator.
    Guest,
}

impl Default for GpuType {
    fn default() -> Self {
        GpuType::Auto
    }
}

display_impl!(GpuType);

/// Selector for the verbosity level of the logs for this instance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Logs will contain entries indicating progress and some configuration details.
    Info,

    /// Logs will contain all entries currently generated by the system. Useful for debugging.
    Verbose,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

display_impl!(LogLevel);

/// Selector for the mode of networking to enable between the guest and host systems.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkingMode {
    /// Networking will be set up in bridged mode, using an interface such as tun/tap.
    Bridged,

    /// Networking will be over explicitly mapped ports, using an interface such as SLiRP.
    Mapped,

    /// Guest networking will be disabled.
    None,
}

impl Default for NetworkingMode {
    fn default() -> Self {
        NetworkingMode::None
    }
}

display_impl!(NetworkingMode);

/// Definition of the CPU type(s) and how many of them to emulate.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct VirtualCpu {
    /// The guest system's CPU architecture, i.e. the CPU type that will be emulated in the guest.
    pub architecture: TargetArchitecture,

    /// The number of virtual CPUs that will emulated in the virtual device.
    pub count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::str::FromStr;

    #[test]
    fn test_accel() -> Result<()> {
        // Verify it returns a default.
        let _default = AccelerationMode::default();
        // Deserialize a valid value.
        assert!(AccelerationMode::from_str("auto").is_ok());
        // Fail to deserialize an invalid value.
        assert!(AccelerationMode::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_console() -> Result<()> {
        // Verify it returns a default.
        let _default = ConsoleType::default();
        Ok(())
    }

    #[test]
    fn test_engine() -> Result<()> {
        // Verify it returns a default.
        let _default = EngineType::default();
        // Deserialize a valid value.
        assert!(EngineType::from_str("qemu").is_ok());
        // Fail to deserialize an invalid value.
        assert!(EngineType::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_gpu() -> Result<()> {
        // Verify it returns a default
        let default = GpuType::default();
        // Verify we can use default formatting to print it.
        println!("{}", default);
        // Deserialize a valid value.
        assert!(GpuType::from_str("auto").is_ok());
        // Fail to deserialize an invalid value.
        assert!(GpuType::from_str("bad_value").is_err());
        Ok(())
    }

    #[test]
    fn test_log() -> Result<()> {
        // Verify it returns a default.
        let _default = LogLevel::default();
        Ok(())
    }

    #[test]
    fn test_net() -> Result<()> {
        // Verify it returns a default.
        let _default = NetworkingMode::default();
        Ok(())
    }

    #[test]
    fn test_cpu() -> Result<()> {
        // Verify it returns a default.
        let _default = VirtualCpu::default();
        Ok(())
    }
}
