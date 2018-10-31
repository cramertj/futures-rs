//! The `select` macro.

/// Polls multiple futures and streams simultaneously, executing the branch
/// for the future that finishes first. Futures and streams passed to
/// `select!` must be `Unpin` and implement `FusedFuture` or `FusedStream`.
/// Futures and streams which are not already fused can be fused using the
/// `.fuse()` method.
///
/// `select!` can select over futures with different output types, but each
/// branch has to have the same return type.
///
/// This macro is only usable inside of async functions, closures, and blocks.
///
/// # Examples
///
/// ```
/// #![feature(pin, async_await, await_macro, futures_api)]
/// # futures::executor::block_on(async {
/// use futures::future::{self, FutureExt};
/// use futures::select;
/// let mut a = future::ready(4);
/// let mut b = future::empty::<()>();
///
/// let res = select! {
///     a_res = a => a_res + 1,
///     _ = b => 0,
/// };
/// assert_eq!(res, 5);
/// # });
/// ```
///
/// In addition to `done(...)` matchers, `select!` includes `next(...)`
/// matchers for getting the next element of a stream:
///
/// ```
/// #![feature(pin, async_await, await_macro, futures_api)]
/// # futures::executor::block_on(async {
/// use futures::future::{self, FutureExt};
/// use futures::stream::{self, StreamExt};
/// use futures::select;
/// let mut st = stream::iter(vec![2]).fuse();
/// let mut fut = future::empty::<()>();
///
/// select! {
///     x => x) => assert_eq!(Some(2), x),
///     done(fut => _) => panic!(),
/// };
/// # });
/// ```
///
/// `select` also accepts a `complete` branch and a `default` branch.
/// `complete` will run if all futures and streams have already been
/// exhausted. `default` will run if no futures or streams are
/// immediately ready.
///
/// ```
/// #![feature(pin, async_await, await_macro, futures_api)]
/// # futures::executor::block_on(async {
/// use futures::future::{self, FutureExt};
/// use futures::select;
/// let mut a = future::ready(4);
/// let mut b = future::ready(6);
/// let mut total = 0;
///
/// loop {
///     select! {
///         done(a => a) => total += a,
///         done(b => b) => total += b,
///         default => panic!(), // never runs (futures run first, then complete)
///         complete => break,
///     };
/// }
/// assert_eq!(total, 10);
/// # });
/// ```
///
/// Note that the futures that have been matched over can still be mutated
/// from inside the `select!` block's branches. This can be used to implement
/// more complex behavior such as timer resets or writing into the head of
/// a stream.
#[macro_export]
macro_rules! select {
    () => {
        compile_error!("The `select!` macro requires at least one branch")
    };

	(
        @codegen
        futs $fut:tt
        default $default:tt
        complete ()
    ) => {
        $crate::select! {
            @codegen
            futs $fut
            default $default
            complete (
                panic!("all futures in select! were completed, \
                       but no `complete =>` handler was provided"))
        }
    };
    // Remember the lack of a default
    (
        @codegen
        futs $fut:tt
        default ()
        complete $complete:tt
    ) => {
        $crate::select! {
            @codegen
            futs $fut
            default ( unreachable!() )
            no_default true
            complete $complete
        }
    };

    (
        @codegen
        futs ($( $fut_pat:pat = $fut_name:ident => $fut_body:expr, )*)
        default ($default:expr)
        $( no_default $no_default:ident )*
        complete ($complete:expr)
    ) => { {

        // Require all arguments to be `Unpin` so that we don't have to pin them,
        // allowing uncompleted futures to be reused by the caller after the
        // `select!` resolves.
        //
        // Additionally, require all arguments to implement `FusedFuture` so that
        // we can ensure that futures aren't polled after completion by use
        // in successive `select!` statements.
        $(
            $crate::async_await::assert_unpin(&$fut_name);
            $crate::async_await::assert_fused_future(&$fut_name);
        )*

        #[allow(bad_style)]
        enum __PrivResult<$($fut_name,)*> {
            $(
                $fut_name($fut_name),
            )*
            __Default,
			__Complete,
        }

        let mut __poll_fn = |lw: &$crate::core_reexport::task::LocalWaker| {
            let mut __any_polled = false;

            $(
                let mut $fut_name = move |lw: &_| {
                    if $crate::async_await::FusedFuture::is_terminated(& $fut_name) {
                        None
                    } else {
                        Some($crate::core_reexport::future::Future::poll(
                            $crate::core_reexport::pin::Pin::new(&mut $fut_name),
                            lw,
                        ).map(__PrivResult::$fut_name))
                    }
                };
                let $fut_name:
                    &mut dyn FnMut(&$crate::core_reexport::task::LocalWaker)
                    -> Option<$crate::core_reexport::task::Poll<_>>
                    = &mut $fut_name;
            )*
            let mut __select_arr = [$( $fut_name, )*];
            $crate::rand_reexport::Rng::shuffle(
                &mut $crate::rand_reexport::thread_rng(),
                &mut __select_arr,
            );

            for __poller in &mut __select_arr {
                match __poller(lw) {
                    Some(x @ $crate::core_reexport::task::Poll::Ready(_)) =>
                        return x,
                    Some($crate::core_reexport::task::Poll::Pending) => {
                        __any_polled = true;
                    }
                    None => {}
                }
            }

            if !__any_polled {
                $crate::core_reexport::task::Poll::Ready(__PrivResult::__Complete)
            } else {
                $(
                    // only if there isn't a default case:
                    drop($no_default);
                    return $crate::core_reexport::task::Poll::Pending;
                )*
                // only reachable if there is a default case:
                #[allow(unreachable_code)]
                return $crate::core_reexport::task::Poll::Ready(__PrivResult::__Default)
            }
        };
        #[allow(unreachable_code)]
        let __priv_res = loop {
            $(
                // only if there isn't a default case:
                drop($no_default);
                break await!($crate::future::poll_fn(__poll_fn));
            )*
            // only reachable if there is a default case:
            break if let $crate::core_reexport::task::Poll::Ready(x) =
                __poll_fn($crate::task::noop_local_waker_ref())
            {
                x
            } else { unreachable!() };
        };
        match __priv_res {
            $(
                __PrivResult::$fut_name($fut_pat) => {
                    $fut_body
                }
            )*
            __PrivResult::__Default => {
                $default
            }
			__PrivResult::__Complete => {
				$complete
			}
        }
    } };

    // All tokens have been successfully parsed into separate cases--
    // continue to individual case parsing and separation.
    (@parse_list
        cases($($head:tt)*)
        tokens()
    ) => {
        $crate::select!(
            @parse_case
            futs()
            default()
            complete()
            cases($($head)*)
            labels(
                case1
                case2
                case3
                case4
                case5
                case6
                case7
                case8
                case9
                case10
                case11
                case12
                case13
                case14
                case15
                case16
                case17
                case18
                case19
                case20
                case21
                case22
                case23
                case24
                case25
                case26
                case27
                case28
                case29
                case30
                case31
            )
        )
    };

    // tweak `default` and `complete` branches to make them more closely
    // resemble `val = fut => { ... }` syntax. This allows us to unify the
    // comma handling, error handling etc.
    (@parse_list
        cases($($head:tt)*)
        tokens(default => $($tail:tt)*)
    ) => {
        $crate::select!(
            @parse_list
            cases($($head)*)
            tokens(__default__ = default => $($tail)*)
        )
    };
    (@parse_list
        cases($($head:tt)*)
        tokens(complete => $($tail:tt)*)
    ) => {
        $crate::select!(
            @parse_list
            cases($($head)*)
            tokens(__complete__ = complete => $($tail)*)
        )
    };

    // The first case is separated by a comma.
    (@parse_list
        cases($($head:tt)*)
        tokens($fut_pat:pat = $fut_expr:expr => $body:expr, $($tail:tt)*)
    ) => {
        $crate::select!(
            @parse_list
            cases($($head)* $fut_pat = $fut_expr => { $body },)
            tokens($($tail)*)
        )
    };
    // Print an error if there is a semicolon after the block.
    (@parse_list
        cases($($head:tt)*)
        tokens($fut_pat:pat = $fut_expr:expr => $body:block; $($tail:tt)*)
    ) => {
        compile_error!("did you mean to put a comma instead of the semicolon after `}`?")
    };

    // Don't require a comma after the case if it has a proper block.
    (@parse_list
        cases($($head:tt)*)
        tokens($fut_pat:pat = $fut_expr:expr => $body:block $($tail:tt)*)
    ) => {
        $crate::select!(
            @parse_list
            cases($($head)* $fut_pat = $fut_expr => { $body },)
            tokens($($tail)*)
        )
    };
    // Only one case remains.
    (@parse_list
        cases($($head:tt)*)
        tokens($fut_pat:pat = $fut_expr:expr => $body:expr)
    ) => {
        $crate::select!(
            @parse_list
            cases($($head)* $fut_pat = $fut_name => { $body },)
            tokens()
        )
    };
    // Accept a trailing comma at the end of the list.
    (@parse_list
        cases($($head:tt)*)
        tokens($fut_pat:pat = $fut_expr:expr => $body:expr,)
    ) => {
        $crate::select!(
            @parse_list
            ($($head)* $fut_pat = $fut_expr => { $body },)
            ()
        )
    };
    // Diagnose and print an error.
    (@parse_list
        cases($($head:tt)*)
        tokens($(tail:tt)*)
    ) => {
        $crate::select!(@parse_list_error1 $($tail)*)
    };
    // Stage 1: check the case type.
    (@parse_list_error1 default $($tail:tt)*) => {
        $crate::select!(@parse_list_error1 $($tail)*)
    };
    (@parse_list_error1 complete $($tail:tt)*) => {
        $crate::select!(@parse_list_error1 $($tail)*)
    };
    (@parse_list error1 $fut_pat:pat = $fut_name:expr => $($tail:tt)*) => {
        $crate::select!(@parse_list_error => $($tail)*)
    };
    (@parse_list_error1 $t:tt $($tail:tt)*) => {
        compile_error!(concat!(
            "expected one of `pattern = future => expr,`, `default => expr,`, or `complete => expr`, found `",
            stringify!($t),
            "`",
        ))
    };
    (@parse_list_error_2 => $body:expr; $($tail:tt)*) => {
        compile_error!(concat!(
            "did you mean to put a comma instead of the semicolon after `",
            stringify!($body),
            "`?",
        ))
    };
    (@parse_list_error_2 => $($tail:tt)*) => {
        compile_error!("expected expression followed by comma after `=>`")
    };
    (@parse_list_error_2 $($tail:tt)*) => {
        compile_error!("expected `=>` after select! case")
    };
    (@parse_list_error2 $($tail:tt)*) => {
        compile_error!("invalid syntax")
    };

    // Success! all cases were parsed
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete $complete:tt
        cases ()
        labels $labels:tt
    ) => {
        $crate::select!(
            @codegen
            futs $fut
            default $default
            complete $complete
        )
    };
    // Error: no labels left
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete $complete:tt
        cases $cases:tt
        labels ()
    ) => {
        compile_error!("too many cases in a `select!` block")
    };

    (@parse_case
        futs $fut:tt
        default ()
        complete $complete:tt
        cases $cases:tt
        $($tail:tt)*
    ) => {
        compile_error!(concat!("cases: ", stringify!($cases)))
    };

    // Parse `default => { ... }`
    (@parse_case
        futs $fut:tt
        default ()
        complete $complete:tt
        cases (__default__ = default => $body:tt, $($tail:tt)*)
        labels $labels:tt
    ) => {
        $crate::select!(
            @parse_case
            futs $fut
            default ($body)
            complete $complete
            cases ($($tail)*)
            labels $labels
        )
    };
    // Error on multiple `default`s
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete $complete:tt
        cases (def = default => $body:tt, $($tail:tt)*)
        labels $labels:tt
    ) => {
        compile_error!("there can only be one `default` case in a `select!` block")
    };
    // Parse `complete => { ... }`
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete ()
        cases (__complete__ = complete => $body:tt, $($tail:tt)*)
        labels $labels:tt
    ) => {
        $crate::select!(
            @parse_case
            futs $fut
            default $default
            complete ($body)
            cases ($($tail)*)
            labels $labels
        )
    };
    // Error on multiple `complete`s
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete $complete:tt
        cases (__complete__ = complete => $body:tt, $($tail:tt)*)
        labels $labels:tt
        used_labels $used:tt
    ) => {
        compile_error!("there can only be one `complete` case in a `select!` block")
    };
    // Parse `pat = ident => { ... }`
    (@parse_case
        futs ($($futs:tt)*)
        default $default:tt
        complete $complete:tt
        cases ($pat:pat = $ident:ident => $body:tt, $($tail:tt)*)
        labels $labels:tt
    ) => {
        $crate::select!(
            @parse_case
            futs ($($futs)* $pat = $ident => $body,)
            default $default
            complete $complete
            cases ($($tail)*)
            labels $labels
        )
    };
    // Parse `pat = expr => { ... }`
    (@parse_case
        futs ($($futs:tt)*)
        default $default:tt
        complete $complete:tt
        cases ($pat:pat => $fut:expr => $body:tt, $($tail:tt)*)
        labels ($label:tt $($labels:tt)*)
    ) => {
        {
            let mut $label = $fut;
            $crate::select!(
                @parse_case
                futs ($($futs)* $pat = $label => $body,)
                default $default
                complete $complete
                cases ($($tail)*)
                labels ($($labels)*)
            )
        }
    };
    // Catch errors in case parsing
    (@parse_case
        futs $fut:tt
        default $default:tt
        complete $complete:tt
        cases ($case:ident $args:tt => $body:tt, $($tail:tt)*)
        labels $labels:tt
    ) => {
        compile_error!(concat!(
            "expected one of `pattern = future => expr,`, `default`, or `complete`, found `",
            stringify!($case), stringify!($args),
            "`",
        ))
    };

    // Catches a bug within this macro (should not happen).
    (@$($tokens:tt)*) => {
        compile_error!(concat!(
            "internal error in futures select macro: ",
            stringify!(@$($tokens)*),
        ))
    };

    ($($tokens:tt)*) => {
        $crate::select!(
            @parse_list
            cases()
            tokens($($tokens)*)
        )
    };
}
