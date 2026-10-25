//! Unit tests for the serialized environment guard.
//!
//! Every test installs its pre-state through [`Fixture`], which restores the
//! ambient value on drop. Fixtures never hold the lock while a guard under
//! test is alive: the guard's lock is non-reentrant.

use std::env;
use std::ffi::OsString;
use std::time::{Duration, Instant};

use super::{EnvGuard, Fixture, env_try_lock};

#[test]
fn set_then_drop_restores_previous_value() {
    let _fixture = Fixture::install("ENVGUARD_TEST_SET_RESTORE", Some("b"));
    {
        let _guard = EnvGuard::set("ENVGUARD_TEST_SET_RESTORE", "a");
        assert_eq!(
            env::var_os("ENVGUARD_TEST_SET_RESTORE"),
            Some(OsString::from("a"))
        );
    }
    assert_eq!(
        env::var_os("ENVGUARD_TEST_SET_RESTORE"),
        Some(OsString::from("b"))
    );
}

#[test]
fn set_then_drop_restores_absence() {
    let _fixture = Fixture::install("ENVGUARD_TEST_SET_ABSENT", None);
    {
        let _guard = EnvGuard::set("ENVGUARD_TEST_SET_ABSENT", "temporary");
        assert_eq!(
            env::var_os("ENVGUARD_TEST_SET_ABSENT"),
            Some(OsString::from("temporary"))
        );
    }
    assert_eq!(env::var_os("ENVGUARD_TEST_SET_ABSENT"), None);
}

#[test]
fn remove_then_drop_restores_previous_value() {
    let _fixture = Fixture::install("ENVGUARD_TEST_REMOVE_RESTORE", Some("x"));
    {
        let _guard = EnvGuard::remove("ENVGUARD_TEST_REMOVE_RESTORE");
        assert_eq!(env::var_os("ENVGUARD_TEST_REMOVE_RESTORE"), None);
    }
    assert_eq!(
        env::var_os("ENVGUARD_TEST_REMOVE_RESTORE"),
        Some(OsString::from("x"))
    );
}

#[test]
fn multi_key_single_guard_restores_all_in_reverse() {
    let _fixture_a = Fixture::install("ENVGUARD_TEST_MULTI_A", Some("oldA"));
    let _fixture_b = Fixture::install("ENVGUARD_TEST_MULTI_B", None);
    let _fixture_c = Fixture::install("ENVGUARD_TEST_MULTI_C", Some("presentC"));
    {
        let mut guard = EnvGuard::acquire();
        guard
            .and_set("ENVGUARD_TEST_MULTI_A", "1")
            .and_set("ENVGUARD_TEST_MULTI_B", "2")
            .and_remove("ENVGUARD_TEST_MULTI_C");
        assert_eq!(
            env::var_os("ENVGUARD_TEST_MULTI_A"),
            Some(OsString::from("1"))
        );
        assert_eq!(
            env::var_os("ENVGUARD_TEST_MULTI_B"),
            Some(OsString::from("2"))
        );
        assert_eq!(env::var_os("ENVGUARD_TEST_MULTI_C"), None);
    }
    assert_eq!(
        env::var_os("ENVGUARD_TEST_MULTI_A"),
        Some(OsString::from("oldA"))
    );
    assert_eq!(env::var_os("ENVGUARD_TEST_MULTI_B"), None);
    assert_eq!(
        env::var_os("ENVGUARD_TEST_MULTI_C"),
        Some(OsString::from("presentC"))
    );
}

#[test]
fn repeated_same_key_mutations_restore_original() {
    let _fixture = Fixture::install("ENVGUARD_TEST_REPEAT", Some("orig"));
    {
        let mut guard = EnvGuard::acquire();
        guard
            .and_set("ENVGUARD_TEST_REPEAT", "1")
            .and_set("ENVGUARD_TEST_REPEAT", "2")
            .and_remove("ENVGUARD_TEST_REPEAT");
        assert_eq!(env::var_os("ENVGUARD_TEST_REPEAT"), None);
    }
    assert_eq!(
        env::var_os("ENVGUARD_TEST_REPEAT"),
        Some(OsString::from("orig"))
    );
}

#[test]
fn poisoned_lock_does_not_cascade() {
    let _fixture = Fixture::install("ENVGUARD_TEST_POISON", None);
    let handle = std::thread::spawn(|| {
        let _guard = EnvGuard::set("ENVGUARD_TEST_POISON", "held");
        panic!("poison the env lock on purpose");
    });
    assert!(handle.join().is_err(), "worker thread should have panicked");

    let mut guard = EnvGuard::acquire();
    guard.and_set("ENVGUARD_TEST_POISON", "after-poison");
    assert_eq!(
        env::var_os("ENVGUARD_TEST_POISON"),
        Some(OsString::from("after-poison"))
    );
}

#[test]
fn env_try_lock_reflects_serialization() {
    let guard = EnvGuard::acquire();
    assert!(
        env_try_lock().is_none(),
        "lock held by this guard must not be re-acquired"
    );
    drop(guard);

    // Another test may briefly hold the lock, so retry without sleeping.
    let deadline = Instant::now() + Duration::from_secs(5);
    let probe = loop {
        if let Some(probe) = env_try_lock() {
            break probe;
        }
        assert!(
            Instant::now() < deadline,
            "env lock stayed contended for 5s"
        );
        std::hint::spin_loop();
    };
    drop(probe);
}

#[test]
fn drop_during_unwind_restores() {
    let _fixture = Fixture::install("ENVGUARD_TEST_UNWIND", Some("prior"));

    let result = std::panic::catch_unwind(|| {
        let _guard = EnvGuard::set("ENVGUARD_TEST_UNWIND", "panicking");
        assert_eq!(
            env::var_os("ENVGUARD_TEST_UNWIND"),
            Some(OsString::from("panicking"))
        );
        panic!("unwind through EnvGuard");
    });
    assert!(result.is_err(), "closure should have panicked");
    assert_eq!(
        env::var_os("ENVGUARD_TEST_UNWIND"),
        Some(OsString::from("prior"))
    );
}
