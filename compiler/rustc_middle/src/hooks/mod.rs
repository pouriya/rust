use crate::mir;
use crate::query::TyCtxtAt;
use crate::ty::{Ty, TyCtxt};
use rustc_span::DUMMY_SP;

macro_rules! declare_hooks {
    ($($(#[$attr:meta])*hook $name:ident($($arg:ident: $K:ty),*) -> $V:ty;)*) => {

        impl<'tcx> TyCtxt<'tcx> {
            $(
            $(#[$attr])*
            #[inline(always)]
            #[must_use]
            pub fn $name(self, $($arg: $K,)*) -> $V
            {
                self.at(DUMMY_SP).$name($($arg,)*)
            }
            )*
        }

        impl<'tcx> TyCtxtAt<'tcx> {
            $(
            $(#[$attr])*
            #[inline(always)]
            #[must_use]
            #[instrument(level = "debug", skip(self), ret)]
            pub fn $name(self, $($arg: $K,)*) -> $V
            {
                (self.tcx.hooks.$name)(self, $($arg,)*)
            }
            )*
        }

        pub struct Providers {
            $(pub $name: for<'tcx> fn(
                TyCtxtAt<'tcx>,
                $($arg: $K,)*
            ) -> $V,)*
        }

        impl Default for Providers {
            fn default() -> Self {
                Providers {
                    $($name: |_, $($arg,)*| bug!(
                        "`tcx.{}{:?}` cannot be called as `{}` was never assigned to a provider function.\n",
                        stringify!($name),
                        ($($arg,)*),
                        stringify!($name),
                    ),)*
                }
            }
        }

        impl Copy for Providers {}
        impl Clone for Providers {
            fn clone(&self) -> Self { *self }
        }
    };
}

declare_hooks! {
    /// Tries to destructure an `mir::Const` ADT or array into its variant index
    /// and its field values. This should only be used for pretty printing.
    hook try_destructure_mir_constant_for_diagnostics(val: mir::ConstValue<'tcx>, ty: Ty<'tcx>) -> Option<mir::DestructuredConstant<'tcx>>;
}
