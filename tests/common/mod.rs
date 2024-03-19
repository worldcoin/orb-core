#[macro_export]
macro_rules! broker_test {
    ($(#[$attr:meta])* $test_name:ident, $test_impl:ident, $timeout:expr) => {
        $(#[$attr])*
        #[test]
        fn $test_name() {
            struct TestId;
            let test_id = ::std::any::TypeId::of::<TestId>();
            $crate::common::run_broker_test(
                ::std::stringify!($test_name),
                &::std::format!("{test_id:?}"),
                ::std::time::Duration::from_millis($timeout),
                ::std::boxed::Box::pin($test_impl()),
            )
        }
    };
}

use futures::prelude::*;
use orb::{agents, logger};
use std::{
    env,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    process,
    time::Duration,
};
use tokio::{process::Command, runtime, time};

const BROKER_TEST_ID_ENV: &str = "ORB_CORE_BROKER_TEST_ID";

pub fn run_broker_test(
    test_name: &str,
    test_id: &str,
    timeout: Duration,
    f: Pin<Box<dyn Future<Output = ()>>>,
) {
    let test_id = format!("{test_id:?}");
    if env::var(BROKER_TEST_ID_ENV).map_or(false, |var| var == test_id) {
        let result = catch_unwind(AssertUnwindSafe(|| {
            agents::init_processes();
            logger::init::<false>();
            tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap().block_on(f);
        }));
        process::exit(if result.is_ok() { 0 } else { 1 });
    }
    let mut test_runner_args = env::args();
    let mut child_args = Vec::new();
    while let Some(arg) = test_runner_args.next() {
        match arg.as_str() {
            "--bench"
            | "--exclude-should-panic"
            | "--force-run-in-process"
            | "--ignored"
            | "--include-ignored"
            | "--show-output"
            | "--test" => {
                child_args.push(arg);
            }
            "--color" | "-Z" => {
                child_args.push(arg);
                if let Some(arg) = test_runner_args.next() {
                    child_args.push(arg);
                }
            }
            _ => {}
        }
    }
    child_args.push("--quiet".into());
    child_args.push("--test-threads".into());
    child_args.push("1".into());
    child_args.push("--nocapture".into());
    child_args.push("--exact".into());
    child_args.push("--".into());
    child_args.push(test_name.into());
    let result =
        runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let mut child = Command::new(env::current_exe().unwrap())
                .args(&child_args)
                .env(BROKER_TEST_ID_ENV, test_id)
                .env(agents::PROCESS_ARGS_ENV, shell_words::join(&child_args))
                .spawn()
                .unwrap();
            time::timeout(timeout, child.wait()).await.expect("timeouted").unwrap()
        });
    assert!(result.success(), "test failed");
}
