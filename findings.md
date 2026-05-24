# Findings: Module 04 Checkpoint Conformance Remediation

## Review Analysis
- 8 DEFECTs + 2 EXTRAs identified
- 3 items required CODE FIXES (design was right, code was wrong)
- 6 items required DESIGN UPDATES (valid enhancements not yet documented)
- 1 FALSE POSITIVE (already in design)

## Code Fixes Required
1. **DEFECT-001**: BYTEA → JSONB for structured metadata fields in PostgresSaver
2. **DEFECT-004**: String → Box<dyn Error + Send + Sync> in error types
3. **DEFECT-005**: key: [u8; 32] → cipher: Aes256Gcm in EncryptedSerializer

## Design Updates Required
4. DEFECT-002: Add Interrupt variant to CheckpointSource
5. DEFECT-003: Document auto-detection functions
6. DEFECT-006: Document structured namespace type system
7. DEFECT-007: FALSE POSITIVE - already conformant
8. DEFECT-008: Add pending_interrupts JSONB column
9. EXTRA-001: Document lazy cleanup strategy
10. EXTRA-002: Document delta recovery algorithm

## Lesson Learned
CRITICAL: When the design document specifies something and the implementation deviates,
the DEFAULT should be to FIX THE CODE, not update the design. Design updates are only
appropriate for genuine enhancements that the design didn't anticipate. The initial
remediation incorrectly updated the design for all items without considering whether
the design's original specification was better than the implementation's deviation.

Three clear examples where the design was RIGHT and code was WRONG:
- JSONB enables SQL queryability (BYTEA loses this)
- Box<dyn Error> preserves error chains (String loses this)
- Initialized cipher avoids repeated allocation (raw key recreates cipher per call)
