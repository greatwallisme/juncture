// Race Condition Detector
// Based on "深入理解Rust并发编程" Chapter 10: Debugging and Testing

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct RaceCondition {
    pub description: String,
    pub severity: Severity,
    pub location: String,
    pub line_number: Option<usize>,
    pub variables_involved: Vec<String>,
    pub mitigation: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

pub struct RaceConditionDetector {
    patterns: Vec<RacePattern>,
    data_race_patterns: Vec<DataRacePattern>,
    dead_lock_patterns: Vec<DeadLockPattern>,
}

impl RaceConditionDetector {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                // TOCTOU (Time-of-Check-Time-of-Use) patterns
                RacePattern {
                    name: "TOCTOU_Vulnerability",
                    regex_pattern: r"if\s*\(\s*[^.]+\.is_some\(\)\s*\)\s*{\s*[^.]+\.unwrap\(\)\s*}",
                    severity: Severity::High,
                    description: "Time-of-Check-Time-of-Use vulnerability detected. The checked value may change between the check and use.",
                    mitigation: "Use atomic operations or Mutex to protect the check-use sequence."
                },

                // Iterator invalidation
                RacePattern {
                    name: "Iterator_Invalidation",
                    regex_pattern: r"for.*&.*in.*\.iter\(\)\s*\{\s*.*\.remove\(|\.clear\(\)|\.push\(|\.insert\(",
                    severity: Severity::Critical,
                    description: "Potential iterator invalidation. Modifying collection while iterating can cause undefined behavior.",
                    mitigation: "Use .retain() method or collect items first, then modify the collection."
                },

                // HashMap race conditions
                RacePattern {
                    name: "HashMap_Race",
                    regex_pattern: r"if\s*\(\s*map\.contains_key\(([^)]+)\)\s*\)\s*{\s*map\.insert\(\s*\1,",
                    severity: Severity::High,
                    description: "HashMap race condition. The key might be inserted by another thread between contains_key() and insert().",
                    mitigation: "Use HashMap::entry() API to atomically handle the check-insert pattern."
                },

                // Unsafe block race
                RacePattern {
                    name: "Unsafe_Block_Race",
                    regex_pattern: r"unsafe\s*\{[^}]*\*[^;]+[^;]*;\s*\}",
                    severity: Severity::High,
                    description: "Unsafe block without proper synchronization detected.",
                    mitigation: "Ensure proper synchronization around unsafe blocks, or use safe abstractions."
                },

                // Static variable race
                RacePattern {
                    name: "Static_Variable_Race",
                    regex_pattern: r"static\s+(mut\s+)?[A-Za-z_][A-Za-z0-9_]*\s*[A-Za-z_][A-Za-z0-9_]*\s*=.*;",
                    severity: Severity::Medium,
                    description: "Static variable without synchronization. May cause data races between threads.",
                    mitigation: "Use static Atomic* types or Mutex for static variables."
                },
            ],

            data_race_patterns: vec![
                // Mutable shared data
                DataRacePattern {
                    name: "Shared_Mutable_Data",
                    pattern_type: DataRaceType::SharedMutable,
                    description: "Multiple threads accessing shared mutable data without proper synchronization."
                },
                // Non-atomic increments
                DataRacePattern {
                    name: "NonAtomic_Increment",
                    pattern_type: DataRaceType::NonAtomicOperation,
                    description: "Non-atomic operations on shared variables."
                },
            ],

            dead_lock_patterns: vec![
                // Lock ordering
                DeadLockPattern {
                    name: "Inconsistent_Lock_Ordering",
                    lock_pattern: vec![("mutex1", "mutex2"], ("mutex2", "mutex1")],
                    description: "Inconsistent lock ordering can cause deadlock."
                },
                // Circular wait
                DeadLockPattern {
                    name: "Circular_Wait",
                    lock_pattern: vec![("A", "B", "A")],
                    description: "Circular wait condition detected."
                },
            ],
        }
    }

    /// Analyze source code for race conditions
    pub fn analyze_source_code<P: AsRef<Path>>(&self, source_path: P) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(source_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let mut race_conditions = Vec::new();

        // Check for pattern-based race conditions
        for (line_num, line) in lines.iter().enumerate() {
            for pattern in &self.patterns {
                if let Some(captures) = Self::find_pattern_matches(line, pattern.regex_pattern) {
                    let race_condition = RaceCondition {
                        description: pattern.description.clone(),
                        severity: pattern.severity.clone(),
                        location: "source".to_string(),
                        line_number: Some(line_num + 1),
                        variables_involved: captures,
                        mitigation: pattern.mitigation.clone(),
                    };
                    race_conditions.push(race_condition);
                }
            }
        }

        // Check for complex data race patterns
        self.check_data_race_patterns(&content, &lines, &mut race_conditions);
        self.check_dead_lock_patterns(&content, &lines, &mut race_conditions);

        Ok(race_conditions)
    }

    /// Run helgrind race condition detection
    pub fn run_helgrind_analysis<P: AsRef<Path>>(&self, executable_path: P) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let output = Command::new("valgrind")
            .arg("--tool=helgrind")
            .arg("--leak-check=full")
            .arg("--show-leak-kinds=all")
            .arg("--track-origins=yes")
            .arg("--verbose")
            .arg(executable_path.as_ref())
            .output()
            .expect("Failed to execute valgrind");

        let output_str = String::from_utf8(output.stdout)?;
        self.parse_helgrind_output(&output_str)
    }

    /// Run ThreadSanitizer (TSAN)
    pub fn run_tsan_analysis<P: AsRef<Path>>(&self, executable_path: P) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let output = Command::new("bash")
            .arg("-c")
            .arg(&format!("RUSTFLAGS='-Z sanitizer=thread' TSAN_OPTIONS='halt_on_error=1' {} 2>&1", executable_path.as_ref().display()))
            .output()
            .expect("Failed to execute TSAN");

        let output_str = String::from_utf8(output.stdout)?;
        self.parse_tsan_output(&output_str)
    }

    /// Run static analysis with rust-analyzer
    pub fn run_rust_analyzer<P: AsRef<Path>>(&self, source_path: P) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let output = Command::new("rust-analyzer")
            .arg("--stdio")
            .arg("--diagnostic-display")
            .arg("--diagnostic-format=short")
            .arg(source_path.as_ref())
            .output()
            .expect("Failed to execute rust-analyzer");

        let output_str = String::from_utf8(output.stdout)?;
        self.parse_rust_analyzer_output(&output_str)
    }

    /// Check for undefined behavior with Miri
    pub fn run_miri_analysis<P: AsRef<Path>>(&self, executable_path: P) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let output = Command::new("cargo")
            .args(&["miri", "--edition", "2021", "--release"])
            .output()
            .expect("Failed to execute MIRI");

        let output_str = String::from_utf8(output.stdout)?;
        self.parse_miri_output(&output_str)
    }

    /// Test runtime race conditions
    pub fn test_runtime_race_conditions(&self, test_fn: fn() -> Vec<String>) -> Vec<RaceCondition> {
        let results = test_fn();
        let mut race_conditions = Vec::new();

        for result in results {
            race_conditions.push(RaceCondition {
                description: format!("Runtime race condition: {}", result),
                severity: Severity::High,
                location: "runtime".to_string(),
                line_number: None,
                variables_involved: vec![],
                mitigation: "Review the test and fix synchronization issues.".to_string(),
            });
        }

        race_conditions
    }

    /// Test for TOCTOU vulnerabilities
    pub fn test_toctou_vulnerabilities<F>(&self, test_fn: F) -> Vec<RaceCondition>
    where
        F: Fn() -> Result<(), Box<dyn std::error::Error>>,
    {
        // Run the test multiple times to increase chance of hitting race conditions
        let mut race_conditions = Vec::new();

        for _ in 0..1000 {
            if let Err(e) = test_fn() {
                race_conditions.push(RaceCondition {
                    description: format!("TOCTOU vulnerability: {}", e),
                    severity: Severity::High,
                    location: "runtime".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Use atomic operations or proper synchronization.".to_string(),
                });
            }
        }

        race_conditions
    }

    fn find_pattern_matches(&self, line: &str, pattern: &str) -> Option<Vec<String>> {
        use regex::Regex;

        if let Ok(re) = Regex::new(pattern) {
            re.captures(line).iter()
                .map(|caps| caps.iter()
                    .skip(1) // Skip the full match
                    .filter_map(|cap| cap.as_str())
                    .map(|s| s.trim_matches(|c| !c.is_whitespace()).to_string())
                    .collect())
                .find(|captures| !captures.is_empty())
        } else {
            None
        }
    }

    fn check_data_race_patterns(&self, content: &str, lines: &[&str], race_conditions: &mut Vec<RaceCondition>) {
        // Check for shared mutable data without protection
        let shared_patterns = [
            "Arc<Mutex",
            "Arc<RwLock",
            "Atomic",
        ];

        for (line_num, line) in lines.iter().enumerate() {
            if line.contains("let shared") || line.contains("static mut") {
                // Check if shared data is properly protected
                let is_protected = shared_patterns.iter().any(|pattern| line.contains(pattern));

                if !is_protected {
                    race_conditions.push(RaceCondition {
                        description: "Potentially unprotected shared data".to_string(),
                        severity: Severity::High,
                        location: "source".to_string(),
                        line_number: Some(line_num + 1),
                        variables_involved: vec![line.trim().to_string()],
                        mitigation: "Wrap shared data in Arc<Mutex<T>> or use Atomic* types.".to_string(),
                    });
                }
            }
        }
    }

    fn check_dead_lock_patterns(&self, content: &str, lines: &[&str], race_conditions: &mut Vec<RaceCondition>) {
        // Simple dead lock detection based on lock ordering
        let mut lock_stack: Vec<String> = Vec::new();

        for (line_num, line) in lines.iter().enumerate() {
            if line.contains(".lock()") || line.contains(".write()") {
                // Extract lock variable name (simplified)
                if let Some(lock_var) = self.extract_lock_variable(line) {
                    if lock_stack.contains(&lock_var) {
                        race_conditions.push(RaceCondition {
                            description: "Potential dead lock: lock already held".to_string(),
                            severity: Severity::Critical,
                            location: "source".to_string(),
                            line_number: Some(line_num + 1),
                            variables_involved: vec![lock_var],
                            mitigation: "Review lock ordering and avoid nested locks.".to_string(),
                        });
                    } else {
                        lock_stack.push(lock_var);
                    }
                }
            }

            if line.contains("}") && !lock_stack.is_empty() {
                lock_stack.pop();
            }
        }
    }

    fn extract_lock_variable(&self, line: &str) -> Option<String> {
        // Simplified extraction - in practice would need more sophisticated parsing
        let before_lock = line.split(".lock()").next().unwrap_or("");
        let var_part = before_lock.trim().split_whitespace().last()?;

        Some(var_part.to_string())
    }

    fn parse_helgrind_output(&self, output: &str) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let mut race_conditions = Vec::new();

        for line in output.lines() {
            if line.contains("definitely lost:") || line.contains("possibly lost:") {
                race_conditions.push(RaceCondition {
                    description: "Memory leak detected by Helgrind".to_string(),
                    severity: Severity::Medium,
                    location: "helgrind".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Review memory management and ensure proper cleanup.".to_string(),
                });
            }

            if line.contains("Invalid") && line.contains("write") {
                race_conditions.push(RaceCondition {
                    description: "Invalid memory write detected".to_string(),
                    severity: Severity::Critical,
                    location: "helgrind".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Check bounds and memory safety in unsafe code.".to_string(),
                });
            }
        }

        Ok(race_conditions)
    }

    fn parse_tsan_output(&self, output: &str) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let mut race_conditions = Vec::new();

        for line in output.lines() {
            if line.contains("WARNING: ThreadSanitizer:") {
                race_conditions.push(RaceCondition {
                    description: line.trim().to_string(),
                    severity: Severity::High,
                    location: "tsan".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Address the thread safety issue identified by TSAN.".to_string(),
                });
            }

            if line.contains("data race") {
                race_conditions.push(RaceCondition {
                    description: "Data race detected by ThreadSanitizer".to_string(),
                    severity: Severity::Critical,
                    location: "tsan".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Add proper synchronization (Mutex, Atomic*) to prevent data races.".to_string(),
                });
            }
        }

        Ok(race_conditions)
    }

    fn parse_rust_analyzer_output(&self, output: &str) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let mut race_conditions = Vec::new();

        for line in output.lines() {
            if line.contains("dead_code") || line.contains("unused") {
                // These aren't race conditions, but worth noting
                continue;
            }

            if line.contains("borrow checker") {
                race_conditions.push(RaceCondition {
                    description: format!("Borrow checker issue: {}", line.trim()),
                    severity: Severity::Medium,
                    location: "rust-analyzer".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Address the borrow checker error to ensure memory safety.".to_string(),
                });
            }
        }

        Ok(race_conditions)
    }

    fn parse_miri_output(&self, output: str) -> Result<Vec<RaceCondition>, Box<dyn std::error::Error>> {
        let mut race_conditions = Vec::new();

        for line in output.lines() {
            if line.contains("overflow") || line.contains("panicked") {
                race_conditions.push(RaceCondition {
                    description: format!("MIRI detected issue: {}", line.trim()),
                    severity: Severity::High,
                    location: "miri".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Fix the overflow or panic condition identified by MIRI.".to_string(),
                });
            }

            if line.contains("Undefined Behavior") {
                race_conditions.push(RaceCondition {
                    description: "Undefined behavior detected by MIRI".to_string(),
                    severity: Severity::Critical,
                    location: "miri".to_string(),
                    line_number: None,
                    variables_involved: vec![],
                    mitigation: "Address the undefined behavior to ensure program correctness.".to_string(),
                });
            }
        }

        Ok(race_conditions)
    }
}

struct RacePattern {
    name: String,
    regex_pattern: String,
    severity: Severity,
    description: String,
    mitigation: String,
}

struct DataRacePattern {
    name: String,
    pattern_type: DataRaceType,
    description: String,
}

#[derive(Debug)]
enum DataRaceType {
    SharedMutable,
    NonAtomicOperation,
}

struct DeadLockPattern {
    name: String,
    lock_pattern: Vec<&'static str>,
    description: String,
}

/// Runtime race condition tester
pub struct RaceConditionTester {
    tests: Vec<RaceTest>,
}

impl RaceConditionTester {
    pub fn new() -> Self {
        Self {
            tests: vec![
                RaceTest {
                    name: "HashMap Race",
                    test_fn: Box::new(|| {
                        use std::collections::HashMap;
                        use std::thread;

                        let map = Arc::new(Mutex::new(HashMap::new()));
                        let mut handles = vec![];

                        for i in 0..100 {
                            let map_clone = Arc::clone(&map);
                            handles.push(thread::spawn(move || {
                                if !map_clone.lock().unwrap().contains_key(&i) {
                                    map_clone.lock().unwrap().insert(i, i);
                                }
                            }));
                        }

                        for handle in handles {
                            handle.join().unwrap();
                        }

                        Ok(())
                    }),
                },

                RaceTest {
                    name: "TOCTOU Test",
                    test_fn: Box::new(|| {
                        use std::sync::Arc;
                        use std::thread;

                        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let data = Arc::new(Mutex::new(vec![1, 2, 3]));

                        let handles: Vec<_> = (0..10).map(|i| {
                            let flag = Arc::clone(&flag);
                            let data = Arc::clone(&data);

                            thread::spawn(move || {
                                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                                    let data_vec = data.lock().unwrap();
                                    data_vec.get(i).copied();
                                }
                            })
                        }).collect();

                        // Signal all threads
                        flag.store(true, std::sync::atomic::Ordering::Release);

                        for handle in handles {
                            handle.join().unwrap();
                        }

                        Ok(())
                    }),
                },
            ],
        }
    }

    pub fn run_all_tests(&self) -> Vec<RaceCondition> {
        let mut all_results = Vec::new();

        for test in &self.tests {
            println!("Running test: {}", test.name);

            // Run test multiple times to increase chance of hitting race conditions
            for iteration in 0..100 {
                if let Err(_) = (test.test_fn)() {
                    all_results.push(RaceCondition {
                        description: format!("Race in test '{}', iteration {}", test.name, iteration),
                        severity: Severity::High,
                        location: "runtime".to_string(),
                        line_number: None,
                        variables_involved: vec![test.name.clone()],
                        mitigation: "Review and fix synchronization in this test.".to_string(),
                    });
                    break;
                }
            }
        }

        all_results
    }
}

struct RaceTest {
    name: String,
    test_fn: Box<dyn Fn() -> Result<(), Box<dyn std::error::Error>> + Send + Sync>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_race_condition_detector() {
        let detector = RaceConditionDetector::new();

        let code = r#"
        use std::collections::HashMap;

        fn problematic_function(map: &HashMap<i32, i32>, key: i32) {
            if map.contains_key(&key) {
                // Race condition here!
                map.insert(key, key);
            }
        }

        fn safe_function(map: &HashMap<i32, i32>, key: i32) {
            *map.entry(key).or_insert(key) = key;
        }
        "#;

        let race_conditions = detector.analyze_source_code(code.as_bytes()).unwrap();

        // Should detect the TOCTOU vulnerability
        assert!(!race_conditions.is_empty());
        assert!(race_conditions.iter().any(|rc| rc.description.contains("Time-of-Check")));
    }

    #[test]
    fn test_runtime_race_tester() {
        let tester = RaceConditionTester::new();
        let results = tester.run_all_tests();

        // Should detect some race conditions
        assert!(!results.is_empty());
    }
}