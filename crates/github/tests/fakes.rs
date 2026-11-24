//! A stress test for [`support::Fakes::write`]: threads that rewrite and run fakes at the same
//! time must never see `ETXTBSY`, which a write descriptor inherited by a sibling's fork causes.

mod support;

use std::process::Command;

use support::Fakes;

/// Threads racing each other.
const THREADS: usize = 8;
/// Rewrites and runs per thread.
const ROUNDS: usize = 40;

#[test]
fn fakes_rewritten_and_run_across_threads_never_fail_to_exec() {
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            scope.spawn(move || {
                let fakes = Fakes::new(false);
                for round in 0..ROUNDS {
                    let value = format!("{thread}-{round}");
                    fakes.write("git", &format!("echo {value}"));
                    let output = Command::new(&fakes.tools.git)
                        .output()
                        .unwrap_or_else(|err| panic!("cannot run the fake git ({value}): {err}"));
                    assert!(
                        output.status.success(),
                        "the fake git ({value}) failed: {}",
                        output.status
                    );
                    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), value);
                }
            });
        }
    });
}
