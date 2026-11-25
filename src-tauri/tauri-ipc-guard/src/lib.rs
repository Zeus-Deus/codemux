//! Keep Tauri's normal command decoding and response types. Its generated
//! async wrappers schedule command bodies through the invoke resolver, so
//! wrap that resolver to enforce admission when the scheduled work starts.

pub use tauri_core::*;

#[doc(hidden)]
pub use tauri_core::generate_handler as upstream_generate_handler;

#[macro_export]
macro_rules! generate_handler {
    ($($commands:tt)*) => {
        $crate::ipc::guard_handler($crate::upstream_generate_handler![$($commands)*])
    };
}

pub mod ipc {
    use std::cell::RefCell;
    use std::future::Future;
    use std::sync::Arc;
    use tauri_core::utils::acl::resolved::ResolvedCommand;
    use tauri_core::Runtime;

    pub use tauri_core::ipc::*;

    /// Set only by the server when constructing a synthetic IPC request.
    pub const REMOTE_ADMISSION_HEADER: &str = "x-codemux-remote-admission";
    const REVOKED: &str = "web-remote: connection is no longer authorized";

    type Admission = Arc<dyn Fn() -> bool + Send + Sync>;

    thread_local! {
        static REMOTE_ADMISSION: RefCell<Option<Admission>> = const { RefCell::new(None) };
    }

    /// Attach authority to a synchronous call to Tauri's `on_message`.
    /// The generated handler moves a clone into its resolver before queuing
    /// the async future, so the check travels with exactly that invocation.
    pub fn with_remote_admission<T>(
        check: impl Fn() -> bool + Send + Sync + 'static,
        invoke: impl FnOnce() -> T,
    ) -> T {
        struct Restore(Option<Admission>);
        impl Drop for Restore {
            fn drop(&mut self) {
                REMOTE_ADMISSION.with(|scope| scope.replace(self.0.take()));
            }
        }
        let previous = REMOTE_ADMISSION.with(|scope| scope.replace(Some(Arc::new(check))));
        let _restore = Restore(previous);
        invoke()
    }

    #[doc(hidden)]
    pub struct Invoke<R: Runtime> {
        pub message: InvokeMessage<R>,
        pub resolver: GuardedResolver<R>,
        pub acl: Option<Vec<ResolvedCommand>>,
    }

    #[doc(hidden)]
    pub fn guard_handler<R, F>(handler: F) -> impl Fn(tauri_core::ipc::Invoke<R>) -> bool
    where
        R: Runtime,
        F: Fn(Invoke<R>) -> bool,
    {
        move |tauri_core::ipc::Invoke {
                  message,
                  resolver,
                  acl,
              }| {
            let admission = if message.headers().contains_key(REMOTE_ADMISSION_HEADER) {
                let check = REMOTE_ADMISSION.with(|scope| scope.borrow().clone());
                match check {
                    Some(check) if check() => Some(check),
                    _ => {
                        resolver.reject(REVOKED);
                        return true;
                    }
                }
            } else {
                None
            };
            handler(Invoke {
                message,
                resolver: GuardedResolver {
                    resolver,
                    admission,
                },
                acl,
            })
        }
    }

    #[doc(hidden)]
    pub struct GuardedResolver<R: Runtime> {
        resolver: InvokeResolver<R>,
        admission: Option<Admission>,
    }

    impl<R: Runtime> GuardedResolver<R> {
        pub fn respond_async_serialized<F>(self, task: F)
        where
            F: Future<Output = Result<InvokeResponseBody, InvokeError>> + Send + 'static,
        {
            self.resolver.respond_async_serialized(async move {
                if self.admission.is_some_and(|check| !check()) {
                    return Err(InvokeError::from(REVOKED));
                }
                task.await
            });
        }

        pub fn invoke_error(self, error: InvokeError) {
            self.resolver.invoke_error(error);
        }
    }

    // Tauri's generated synchronous wrappers consume the resolver by value.
    // Keep the upstream return-type selection and serialization, with tags
    // that accept our resolver and pass the original through unchanged.
    #[doc(hidden)]
    pub mod private {
        #[cfg(feature = "tracing")]
        pub use tauri_core::ipc::private::tracing;
        pub use tauri_core::ipc::private::{FutureKind, ResultFutureKind};
        use tauri_core::ipc::private::{ResponseTag, ResultTag};
        use tauri_core::ipc::{InvokeError, IpcResponse};
        use tauri_core::Runtime;

        use super::GuardedResolver;

        pub trait ResponseKind {
            fn blocking_kind(&self) -> BlockingResponse {
                BlockingResponse
            }
            fn async_kind(&self) -> ResponseTag {
                ResponseTag
            }
        }

        impl<T: IpcResponse> ResponseKind for &T {}

        pub struct BlockingResponse;

        impl BlockingResponse {
            pub fn block<R: Runtime, T: IpcResponse>(self, value: T, resolver: GuardedResolver<R>) {
                ResponseTag.block(value, resolver.resolver);
            }
        }

        pub trait ResultKind {
            fn blocking_kind(&self) -> BlockingResult {
                BlockingResult
            }
            fn async_kind(&self) -> ResultTag {
                ResultTag
            }
        }

        impl<T: IpcResponse, E: Into<InvokeError>> ResultKind for Result<T, E> {}

        pub struct BlockingResult;

        impl BlockingResult {
            pub fn block<R: Runtime, T: IpcResponse, E: Into<InvokeError>>(
                self,
                value: Result<T, E>,
                resolver: GuardedResolver<R>,
            ) {
                ResultTag.block(value, resolver.resolver);
            }
        }
    }
}
