use std::{
    ffi::{c_char, CStr},
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull,
};

use rustc_data_structures::small_c_str::SmallCStr;

use crate::{errors::LlvmError, llvm};

/// Responsible for safely creating and disposing llvm::TargetMachine via ffi functions.
/// Not cloneable as there is no clone function for llvm::TargetMachine.
#[repr(transparent)]
pub struct OwnedTargetMachine {
    tm_unique: NonNull<llvm::TargetMachine>,
    phantom: PhantomData<llvm::TargetMachine>,
}

impl OwnedTargetMachine {
    pub fn new(
        triple: &CStr,
        cpu: &CStr,
        features: &CStr,
        abi: &CStr,
        model: llvm::CodeModel,
        reloc: llvm::RelocModel,
        level: llvm::CodeGenOptLevel,
        use_soft_fp: bool,
        function_sections: bool,
        data_sections: bool,
        unique_section_names: bool,
        trap_unreachable: bool,
        singletree: bool,
        asm_comments: bool,
        emit_stack_size_section: bool,
        relax_elf_relocations: bool,
        use_init_array: bool,
        split_dwarf_file: &CStr,
        debug_info_compression: &CStr,
        force_emulated_tls: bool,
        args_cstr_buff: &[u8],
    ) -> Result<Self, LlvmError<'static>> {
        assert!(args_cstr_buff.len() > 0);
        assert!(
            *args_cstr_buff.last().unwrap() == 0,
            "The last character must be a null terminator."
        );

        // SAFETY: llvm::LLVMRustCreateTargetMachine copies pointed to data
        let tm_ptr = unsafe {
            llvm::LLVMRustCreateTargetMachine(
                triple.as_ptr(),
                cpu.as_ptr(),
                features.as_ptr(),
                abi.as_ptr(),
                model,
                reloc,
                level,
                use_soft_fp,
                function_sections,
                data_sections,
                unique_section_names,
                trap_unreachable,
                singletree,
                asm_comments,
                emit_stack_size_section,
                relax_elf_relocations,
                use_init_array,
                split_dwarf_file.as_ptr(),
                debug_info_compression.as_ptr(),
                force_emulated_tls,
                args_cstr_buff.as_ptr() as *const c_char,
                args_cstr_buff.len(),
            )
        };

        NonNull::new(tm_ptr)
            .map(|tm_unique| Self { tm_unique, phantom: PhantomData })
            .ok_or_else(|| LlvmError::CreateTargetMachine { triple: SmallCStr::from(triple) })
    }
}

impl Deref for OwnedTargetMachine {
    type Target = llvm::TargetMachine;

    fn deref(&self) -> &Self::Target {
        // SAFETY: constructing ensures we have a valid pointer created by llvm::LLVMRustCreateTargetMachine
        unsafe { self.tm_unique.as_ref() }
    }
}

impl Drop for OwnedTargetMachine {
    fn drop(&mut self) {
        // SAFETY: constructing ensures we have a valid pointer created by llvm::LLVMRustCreateTargetMachine
        // OwnedTargetMachine is not copyable so there is no double free or use after free
        unsafe {
            llvm::LLVMRustDisposeTargetMachine(self.tm_unique.as_mut());
        }
    }
}
