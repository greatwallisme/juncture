"""Shared utilities for Python benchmark scripts.

Provides CPU time and peak RSS measurement via the stdlib ``resource`` module
(matching the Rust profiling in ``benchmarks/src/profiling.rs``).
"""

import resource
import time


def get_cpu_time_ms() -> float:
    """Return process CPU time (user + system) in milliseconds."""
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return (usage.ru_utime + usage.ru_stime) * 1000.0


def get_peak_rss_mb() -> float:
    """Return peak resident set size in megabytes.

    On Linux, ``ru_maxrss`` is in kilobytes.
    """
    usage = resource.getrusage(resource.RUSAGE_SELF)
    return usage.ru_maxrss / 1024.0


def measure_run(func, *args, **kwargs) -> tuple:
    """Run *func* and return ``(result, wall_ms, cpu_ms_delta, peak_rss_mb)``.

    Measures wall-clock time, CPU time delta, and peak RSS around a single call.
    """
    cpu_before = get_cpu_time_ms()
    start = time.perf_counter()
    result = func(*args, **kwargs)
    wall_ms = (time.perf_counter() - start) * 1000.0
    cpu_after = get_cpu_time_ms()
    rss = get_peak_rss_mb()
    return result, wall_ms, cpu_after - cpu_before, rss
