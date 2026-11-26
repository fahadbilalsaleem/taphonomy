# Regression inputs

Each file is an input in the format of `fuzz/fuzz_targets/sparse.rs`. The CI
replay runs every target on every file here (`.github/workflows/ci.yml`).
All four were made by hand for EXP-0012's screen (b), each from a seed
changed for one refusal and resealed. The first three reach a refusal no
fuzzing trial reached. The fourth was made for a refusal no trial reached
(line 3566) and reaches the refusal at line 3612, which the trials had
already reached (`ADR-0023`). Line numbers are those of the tree they were
checked against (`0994dd7`; the sites are the same as at `2bc7a76`).

- `analyse_image-17-fat-directory-chain-bound.seed`: a FAT32 directory
  whose cluster chain is one cluster over the bound
  (`DirectoryError::TooLarge`, `src/fat_directory.rs:1082`).
- `exfat_reader-36-texfat.seed`: an exFAT volume with two FATs, which is
  TexFAT (`ExFatError::TexFat`, `src/exfat_boot.rs:812`).
- `exfat_reader-58-live-clusters-bound.seed`: an exFAT volume whose live
  files and directories hold more clusters than the bound
  (`ExFatStructureError::LiveClustersBeyondBound`,
  `src/exfat_directory.rs:2136`).
- `exfat_reader-92-chain-bad-cluster.seed`: made for the refusal of a
  directory chain that runs into a bad cluster when that cluster is
  entered (`ExFatDirectoryError::BadCluster`, `src/exfat_directory.rs:3566`).
  It reaches the link check instead, at `src/exfat_directory.rs:3612`
  (canary #98), a refusal the trials had already reached.
