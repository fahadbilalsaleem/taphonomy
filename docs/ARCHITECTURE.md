# Taphonomy Architecture

> **Status notice:** This document describes an intended architecture. A
> section becomes binding only when corresponding code and tests exist.
> Section 47 records which sections that is true of, as of 2026-10-20.

**Project:** Taphonomy
**Document:** Architecture Specification
**Version:** 0.1.0
**Status:** Partly implemented; see section 47
**Last Updated:** 2026-10-20

---

## 1. Purpose

This document defines the architectural boundaries and invariants of Taphonomy.

The architecture is intentionally incomplete at this stage.

Taphonomy is a recovery platform covering multiple fundamentally different data sources. The architecture must allow these domains to evolve independently without creating a monolithic recovery engine.

Implementation details will be established through research, experiments, and Architecture Decision Records.

---

# 2. Architectural Goals

The architecture must support:

1. Evidence preservation.
2. Read-only source handling.
3. Independent recovery domains.
4. Deterministic analysis.
5. Reproducible testing.
6. Strong separation of concerns.
7. Local-first execution.
8. Defensive processing of untrusted binary data.
9. Auditable operations.
10. Incremental expansion.
11. Platform-specific implementations where necessary.
12. Replaceable recovery algorithms.

---

# 3. Architectural Non-Goals

The initial architecture will not attempt to:

* support every filesystem
* support every mobile device
* provide a graphical interface
* provide a web interface
* provide cloud recovery
* create a universal abstraction that hides all filesystem differences
* expose every internal capability through the CLI
* predict recoverability without evidence
* guarantee recovery

---

# 4. High-Level Architecture

The conceptual architecture is:

```text
                         TAPHONOMY
                             │
                     ┌───────┴───────┐
                     │               │
                    CLI          Future UI
                     │
                     ▼
             Application Layer
                     │
       ┌─────────────┼─────────────┐
       │             │             │
       ▼             ▼             ▼
   Evidence      Analysis      Recovery
   Management      Engine        Engine
       │             │             │
       │             │      ┌──────┼──────────┐
       │             │      │      │          │
       │             │      ▼      ▼          ▼
       │             │   Storage  Mobile   Application
       │             │   Recovery Recovery Recovery
       │             │
       └─────────────┴──────────────┐
                                    ▼
                              Verification
                                    │
                                    ▼
                               Reporting
```

This is a conceptual boundary model, not a final module layout.

---

# 5. Core Architectural Domains

Taphonomy is divided into distinct conceptual domains.

## 5.1 Device Discovery

Responsible for identifying accessible sources and their characteristics.

Potential responsibilities:

* enumerate devices
* identify device type
* identify capacity
* identify vendor/model
* identify stable identifiers
* identify mount state
* identify partitions
* report access capabilities

Device discovery must not perform recovery.

---

## 5.2 Evidence Management

Responsible for representing and controlling evidence.

Potential responsibilities:

* source identity
* evidence identity
* acquisition metadata
* image metadata
* hashes
* evidence manifests
* integrity verification
* evidence state

Evidence management must not implement filesystem-specific recovery algorithms.

---

## 5.3 Acquisition

Responsible for copying source evidence into a controlled working representation.

Potential responsibilities:

* raw imaging
* segmented imaging
* acquisition progress
* acquisition errors
* hashing
* verification
* interruption handling

Acquisition must not interpret filesystem semantics unless required for a documented acquisition feature.

---

## 5.4 Analysis

Responsible for understanding the structure of working evidence.

Potential responsibilities:

* partition detection
* filesystem identification
* metadata inspection
* filesystem structure analysis
* block/cluster mapping
* allocation analysis

Analysis must not modify source evidence.

---

## 5.5 Recovery

Responsible for reconstructing or extracting artifacts from analyzed evidence.

Potential responsibilities:

* deleted-file recovery
* metadata reconstruction
* file carving
* fragmented-file reconstruction
* artifact extraction

Recovery algorithms must operate against controlled evidence rather than directly modifying the source.

---

## 5.6 Validation

Responsible for determining whether a recovered artifact is internally consistent and what can be established about its integrity.

Potential responsibilities:

* structural validation
* size validation
* format validation
* hash calculation
* metadata consistency
* reconstruction confidence

Validation must remain separate from extraction.

Successfully extracting bytes does not automatically mean those bytes represent a valid recovered artifact.

---

## 5.7 Reporting

Responsible for presenting operation results.

Potential responsibilities:

* recovery reports
* operation summaries
* errors
* warnings
* confidence
* evidence references
* hashes
* timestamps
* diagnostic information

Reporting must not alter evidence.

---

# 6. Recovery Domains

Recovery implementations are divided by source technology.

```text
Recovery
│
├── Removable Storage
│   ├── USB
│   ├── SD
│   └── microSD
│
├── Magnetic Storage
│   └── HDD
│
├── Solid-State Storage
│   └── SSD
│
├── Mobile
│   ├── Android
│   └── iOS
│
└── Application Artifacts
    ├── SQLite
    ├── Messaging
    ├── Backups
    └── Application-specific formats
```

These domains must not be forced into a single recovery implementation when their underlying technologies differ.

---

# 7. Filesystem Implementations

Filesystem-specific functionality must be isolated behind explicit boundaries.

Potential future implementations include:

```text
FAT32
exFAT
NTFS
ext4
HFS+
APFS
```

The actual support list will be determined through research.

A filesystem implementation may provide:

* superblock/boot-sector parsing
* allocation structures
* directory structures
* metadata structures
* deleted-entry analysis
* block/cluster mapping

Filesystem-specific logic must not leak unnecessarily into unrelated recovery domains.

---

# 8. Storage Medium vs Filesystem

Taphonomy must distinguish between:

```text
STORAGE MEDIUM
```

and:

```text
FILESYSTEM
```

For example:

```text
microSD
    └── exFAT
```

and:

```text
HDD
    └── NTFS
```

are different dimensions.

Recovery behavior may depend on both.

This distinction prevents storage-media-specific assumptions from becoming embedded in filesystem implementations.

---

# 9. HDD and SSD Separation

HDD and SSD recovery must remain separate architectural domains.

An HDD generally exposes a different physical storage model from an SSD.

SSD behavior can involve:

* flash translation layers
* wear leveling
* garbage collection
* TRIM
* controller-level behavior
* encryption

The architecture must not assume that a logical block-addressed interface represents persistent physical storage in the same way across media types.

---

# 10. Mobile Separation

Mobile recovery must remain separate from removable-storage recovery.

Android and iOS introduce additional layers including:

* device security
* encryption
* operating-system restrictions
* application sandboxing
* backups
* acquisition methods
* hardware-specific behavior

Mobile functionality should therefore have its own acquisition and analysis boundaries.

---

# 11. Application Artifact Separation

Application data recovery must not be tightly coupled to a particular physical device.

For example:

```text
SQLite database
```

may originate from:

```text
Android
iOS
Windows
Linux
macOS
backup
disk image
```

The SQLite parser should not need to understand how the database was acquired.

Acquisition and artifact analysis should remain separate.

---

# 12. Data Flow

The preferred recovery data flow is:

```text
Source
  │
  │ read-only
  ▼
Acquisition
  │
  ▼
Evidence Image
  │
  │ integrity verification
  ▼
Analysis
  │
  ▼
Recovery
  │
  ▼
Validation
  │
  ▼
Recovered Artifact
  │
  ▼
Report
```

The source should not normally appear on the right side of the acquisition boundary.

---

# 13. Dependency Direction

The architecture should generally follow:

```text
Interface
    ↓
Application
    ↓
Domain
    ↓
Infrastructure
```

Higher-level components may depend on lower-level abstractions.

Lower-level components must not depend on the CLI or user interface.

For example:

```text
Filesystem Parser
```

must not depend on:

```text
CLI argument parsing
```

A recovery algorithm should be callable from tests without invoking the CLI.

---

# 14. Domain Independence

A failure in one recovery domain should not compromise unrelated domains.

For example:

```text
APFS parser failure
```

must not corrupt:

```text
FAT32 recovery
```

Similarly:

```text
Android artifact analysis
```

must not require:

```text
NTFS support
```

Modules should have narrow responsibilities and explicit interfaces.

---

# 15. Evidence Boundary

Evidence access must have a defined boundary.

Conceptually:

```text
┌───────────────────────────────┐
│        Evidence Layer         │
│                               │
│ source / image / reader       │
│ hashing / identity / bounds   │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│       Analysis Layer          │
│                               │
│ parsers / structures / maps   │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│       Recovery Layer          │
│                               │
│ reconstruction / carving      │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│       Validation Layer        │
└───────────────┬───────────────┘
                │
                ▼
             Output
```

No higher-level layer should gain uncontrolled access to the physical source.

---

# 16. Reader Abstraction

Analysis should eventually operate through a controlled read interface.

Conceptually:

```text
EvidenceReader
    │
    ├── read(offset, length)
    ├── size()
    └── metadata()
```

The exact API will be determined during implementation.

The important invariant is that analysis should not need to know whether the bytes came from:

```text
physical device
raw image
segmented image
memory-backed fixture
test file
```

This enables deterministic testing.

---

# 17. Virtual Evidence

The same analysis engine should eventually be capable of operating on:

```text
physical evidence
disk images
test fixtures
synthetic images
```

This allows recovery algorithms to be tested without exposing them to real user data.

---

# 18. Output Boundary

Recovery output must be isolated from source evidence.

Conceptually:

```text
Evidence
   │
   ▼
Analysis
   │
   ▼
Recovery
   │
   ▼
Output Writer
   │
   ▼
Recovery Destination
```

The output writer must enforce:

* destination boundaries
* path safety
* overwrite protection
* output integrity
* metadata recording

---

# 19. Error Model

Errors should be structured rather than represented solely by human-readable strings.

Conceptual categories:

```text
DeviceError
EvidenceError
AcquisitionError
FilesystemError
ParserError
RecoveryError
ValidationError
OutputError
ConfigurationError
SecurityError
```

Errors should preserve sufficient context for diagnostics without exposing sensitive data.

---

# 20. Result Model

Recovery operations should return structured results.

A conceptual result may contain:

```text
operation_id
source_id
evidence_id
method
status
artifact
size
offset
hash
validation
confidence
warnings
errors
```

The exact schema will be defined after implementation research.

---

# 21. Operation Identity

Important operations should have unique identifiers.

Example:

```text
operation_id: 01J...
```

This allows logs, reports, manifests, and errors to be correlated without relying solely on filenames or timestamps.

---

# 22. Determinism

Where practical:

```text
same evidence
+
same Taphonomy version
+
same configuration
=
same result
```

Nondeterministic behavior must be documented.

Parallel processing may change execution order but should not change the final logical result.

---

# 23. Concurrency

Concurrency should be introduced only when:

1. correctness is established
2. source access remains safe
3. output ordering is controlled
4. resource usage is bounded
5. deterministic results remain possible

Performance optimizations must not compromise evidence integrity.

---

# 24. State Management

Taphonomy should avoid unnecessary global mutable state.

Operations should explicitly receive the resources and configuration they require.

This improves:

* testability
* reproducibility
* concurrency safety
* debugging
* dependency isolation

---

# 25. Configuration

Configuration must have explicit precedence.

The eventual model should distinguish between:

```text
built-in safe defaults
user configuration
command-line overrides
operation-specific configuration
```

Unsafe defaults must not be hidden inside configuration files.

---

# 26. CLI Boundary

The CLI is an interface to Taphonomy, not the recovery engine itself.

Conceptually:

```text
CLI
 │
 ▼
Application Service
 │
 ▼
Domain
```

The CLI should translate user intent into validated operations.

It must not contain filesystem recovery algorithms.

---

# 27. Future User Interfaces

A future desktop or web interface should call the same application/domain layer used by the CLI.

The architecture must not require recovery functionality to be duplicated for:

```text
CLI
Desktop
Web
```

The CLI remains the initial interface.

---

# 28. Local-First Architecture

The core system should be capable of operating entirely offline.

The minimum recovery path should not require:

* cloud storage
* remote APIs
* telemetry
* external authentication
* remote databases

Local storage is the default trust boundary.

---

# 29. External Tool Boundary

Existing recovery utilities may be integrated in the future.

They must remain external components.

Conceptually:

```text
Taphonomy
    │
    ▼
External Tool Adapter
    │
    ▼
External Tool
```

External tools must not gain uncontrolled access to arbitrary evidence.

Their versions, inputs, outputs, privileges, and limitations must be documented.

---

# 30. Plugin Model

A plugin architecture is not currently approved.

We will not create a plugin system merely to appear extensible.

If future requirements demonstrate that plugins provide meaningful architectural value, the plugin boundary will be designed through a separate ADR.

Until then, native modules are preferred.

---

# 31. Database Usage

A server database is not required for the core recovery engine.

If persistent metadata storage becomes necessary, local embedded storage may be evaluated.

A database must not become a prerequisite for basic evidence acquisition or analysis.

---

# 32. Web Application

A web interface is not part of the initial architecture.

If introduced later, it must not require uploading evidence to a server.

A future local web interface could operate against a local Taphonomy process, but this will be evaluated separately.

---

# 33. Desktop Application

A desktop application is a future interface concern.

The recovery engine should remain independent of:

* Electron
* Tauri
* Qt
* native Windows UI
* browser rendering

The initial implementation should therefore remain CLI-first.

---

# 34. Security Boundary

Security-sensitive functionality must be isolated.

Examples:

```text
device access
privileged operations
external process execution
filesystem writes
archive extraction
file parsing
```

Each boundary should have explicit validation and error handling.

---

# 35. Recovery Algorithm Boundary

A recovery algorithm should receive controlled evidence and produce structured candidate artifacts.

Conceptually:

```text
EvidenceReader
      │
      ▼
Recovery Algorithm
      │
      ▼
Candidate Artifact
      │
      ▼
Validator
      │
      ▼
Recovery Result
```

The recovery algorithm should not directly decide whether a result is trustworthy.

Validation remains a separate responsibility.

---

# 36. Candidate vs Verified Artifact

Taphonomy should distinguish between:

```text
Candidate
```

and:

```text
Verified Artifact
```

A recovery algorithm may discover bytes that appear to represent a file.

The validator determines whether the evidence supports a stronger claim.

This separation is fundamental to preventing false-positive recovery.

`docs/decisions/ADR-0003-recovery-confidence-model.md` section 3.1 defines
this Candidate/Artifact boundary as the pipeline it names. A Candidate
carries no confidence level because it has not been assessed; an Artifact
carries exactly one. The distinction drawn here is that pipeline boundary
and is not itself a confidence classification.

---

# 37. Observability

Operations should expose structured diagnostic information.

Potential observability data:

* operation ID
* stage
* progress
* source identity
* offsets
* bytes processed
* errors
* warnings
* timing
* result counts

Sensitive recovered content must not be included by default.

---

# 38. Testing Architecture

Testing must occur at multiple levels.

```text
Unit
  ↓
Component
  ↓
Integration
  ↓
Filesystem Fixture
  ↓
End-to-End
```

Where appropriate, fuzz testing will be added for parsers.

Tests should operate against controlled evidence.

---

# 39. Test Fixture Architecture

Fixtures should represent known states.

For example:

```text
original filesystem
       ↓
controlled deletion
       ↓
known damaged filesystem
       ↓
Taphonomy
       ↓
expected result
```

Each fixture should document how it was generated and what result is expected.

---

# 40. Recovery Laboratory

The project will maintain a controlled recovery laboratory.

Its purpose is to evaluate:

* recovery accuracy
* false positives
* false negatives
* filesystem behavior
* fragmentation
* metadata corruption
* parser robustness
* performance
* resource consumption

The recovery laboratory is a first-class engineering component, not an ad-hoc collection of files.

---

# 41. Versioning

Taphonomy's behavior may depend on:

* filesystem algorithms
* parser implementations
* recovery heuristics
* validation rules

Recovery results should therefore be associated with the Taphonomy version used to produce them.

A future report should be able to answer:

```text
Which version produced this artifact?
```

---

# 42. Compatibility

Compatibility must be documented rather than assumed.

The project should eventually define:

```text
supported operating systems
supported architectures
supported filesystems
supported device classes
supported image formats
supported artifact formats
```

Unsupported combinations should produce explicit errors.

---

# 43. Architecture Evolution

The architecture is expected to change as research reveals new requirements.

Architectural changes must:

1. identify the problem
2. document the existing behavior
3. evaluate alternatives
4. create or update an ADR
5. implement the change
6. update tests
7. update documentation
8. remove obsolete structures

Architecture must not accumulate obsolete abstractions merely for backward compatibility.

---

# 44. Architectural Invariants

The following invariants are considered foundational:

### Invariant 1

Analysis must not require modification of source evidence.

### Invariant 2

Recovery output must be separated from source evidence.

### Invariant 3

Filesystem-specific logic must remain isolated.

### Invariant 4

Device acquisition must remain separate from artifact analysis.

### Invariant 5

Extraction and validation must remain separate.

### Invariant 6

The core engine must be capable of operating locally.

### Invariant 7

Untrusted evidence must never be treated as trusted input.

### Invariant 8

The CLI must not contain domain recovery logic.

### Invariant 9

Recovery domains must be independently testable.

### Invariant 10

Architectural changes must be documented.

---

# 45. Current Architectural Decision

At version 0.1.0, Taphonomy adopts a:

> **Local-first, evidence-preserving, modular, layered architecture with independent recovery domains and controlled evidence access.**

This is the architectural baseline.

Specific technologies, APIs, module names, filesystem implementations, and optimization strategies remain subject to research and future Architecture Decision Records.

---

# 46. Architecture Status

This document describes architectural boundaries, not a completed implementation.

No component described here should be interpreted as implemented unless corresponding code, tests, and documentation exist in the repository.

---

# 47. Implementation Map

Which sections of this document now have corresponding code, as of
2026-10-20. Where a section is marked built, it is binding; where it is not,
it remains intent. `ADR-0011` Decision A defines the distinction.

**Built.**

| Section | Where |
| --- | --- |
| 5.2 Evidence Management | `src/evidence.rs` opens evidence read-only; `src/hash.rs` computes SHA-256 (`ADR-0004`) |
| 5.4 Analysis | `src/partition.rs` (MBR, `ADR-0005`), `src/filesystem.rs`, `src/fat.rs`, `src/fat32.rs`, `src/fat_directory.rs`; exFAT in `src/exfat_boot.rs`, `src/exfat_directory.rs` and `src/analysis/exfat.rs` (`ADR-0019`, `ADR-0021`) |
| 5.5 Recovery | `src/fat_recovery.rs` assesses and reads a deleted file (`ADR-0010`); `src/extraction.rs` hashes and writes the artifact (`ADR-0015`, `ADR-0020`) |
| 15 Evidence Boundary | `EvidenceFile`; asserted by `tests/read_only.rs` |
| 16 Reader Abstraction | The `EvidenceReader` trait, `read_exact_at` (`ADR-0007`) |
| 18 Output Boundary | A destination holding the evidence is refused; each artifact is read back after writing (`ADR-0015`) |
| 19 Error Model | Typed errors: `Error`, `ParseError`, `DirectoryError`, `RecoveryError`, `DigestParseError`, `ExFatError`, `ExFatBootFailure`, `ExFatDirectoryError`, `ExFatContentError`, `SetRefusal` |
| 22 Determinism | Fixtures byte-identical on every build (`ADR-0006`, `scripts/verify-fixtures.sh`) |
| 23 Concurrency | None is introduced; the tool is single-threaded |
| 24 State Management | No global mutable state |
| 28 Local-First | One dependency, `sha2`; nothing reaches a network |
| 36 Candidate vs Verified Artifact | `src/confidence.rs` (`ADR-0003`, `ADR-0014`) |
| 38, 39, 40 Testing, Fixtures, Laboratory | Unit tests, fourteen integration harnesses, generated fixtures, `docs/development/EXPERIMENTS.md` |

**In part.**

| Section | What exists, and what does not |
| --- | --- |
| 5.6 Validation | Comparison against an operator's reference digest (`ADR-0013`); no structural validation |
| 5.7 Reporting | A text report on standard output, with coverage (`ADR-0014`); no structured report |
| 7 Filesystem Implementations | FAT32 is analysed and recovered from; exFAT is analysed read-only, with deleted files not yet examined; FAT12, FAT16, NTFS and TexFAT are identified and not analysed |
| 17 Virtual Evidence | Raw disk images and an in-memory test double; no physical or segmented evidence |
| 13, 26 Dependency Direction, CLI Boundary | A binary over a library crate; `src/analysis.rs` (`ADR-0018`) is an application layer between the CLI and the domain modules it calls; no further internal layering exists |
| 20 Result Model | An event stream and counts, `taphonomy::analysis::Event` and `RunCounts` (`ADR-0018`); no operation identity and no serialised result |

**Not built.** 5.1 Device Discovery; 5.3 Acquisition; 6 and 8 to 11, the
domains beyond filesystem recovery from images; 21 Operation Identity;
25 Configuration beyond command-line arguments; 27 and 29 to 33, other
interfaces, external tools, plugins, databases, web and desktop; 37
Observability; 41 Versioning; 42 Compatibility.

**Invariants.** 1, 2 and 6 hold for the reasons in the tables above. 5
holds: recovery is `src/fat_recovery.rs` and `src/extraction.rs`, and
comparison against a reference is `src/validation.rs`. 10 holds: every
capability has a decision record in `docs/decisions`. 3 and 9 hold for the
library. 4 has nothing to act on yet, since there is no acquisition. 7 holds
as far as parsing is bounds-checked; no fuzz testing exists (`SECURITY.md`
section 36).

**Invariant 8 holds.** `ADR-0018`. The partition loop, the directory walk,
the orphan search and the run's counts are `src/analysis.rs`, called from
`src/main.rs` through `taphonomy::analysis::run` and a `Sink` the CLI
implements to render its events. `src/main.rs` keeps argument handling, the
checks on an output directory, opening and hashing the evidence, rendering,
and the exit status.
