#!/usr/bin/env python3
"""Run the existing reporting tools on each damaged case and record what they print.

Usage: python3 -I gpt-damage-tools.py CASEDIR OUTDIR
  CASEDIR holds c00.img to c25.img (from gpt-damage-cases.py). OUTDIR must
  not exist; the script refuses one that does, and exits 1 on any failure.

Per case and tool: sgdisk -v, sgdisk -p, fdisk -l, partx --show,
blkid -p -o export, mmls. Each runs with no terminal input and a 60 s timeout.
Writes OUTDIR/out/<case>.<tool>.txt (command, stdout, stderr, exit status),
OUTDIR/versions.txt and OUTDIR/summary.tsv. No Taphonomy run here.
"""
import os
import re
import subprocess
import sys

CASE_IDS = [f"c{n:02d}" for n in range(26)]
TIMEOUT = 60
TOOLS = {
    "sgdisk-v": ["sgdisk", "-v"],
    "sgdisk-p": ["sgdisk", "-p"],
    "fdisk-l": ["fdisk", "-l"],
    "partx": ["partx", "--show"],
    "blkid": ["blkid", "-p", "-o", "export"],
    "mmls": ["mmls"],
}
VERSION_COMMANDS = {
    "sgdisk": ["sgdisk", "--version"],
    "fdisk": ["fdisk", "--version"],
    "partx": ["partx", "--version"],
    "blkid": ["blkid", "-v"],
    "mmls": ["mmls", "-V"],
}
ENV = {"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"}

# Parsing: the partition and warning rules of the first task's summarize.py,
# applied to the output formats observed for these tools.
PARA_START = re.compile(
    r"^(Caution|Warning|WARNING|ERROR|Error|\*{4}|Creating new GPT entries|Verification may miss)"
    r"|^The primary GPT table is corrupt"
    r"|^GPT detected: not supported|^\s+GPT not analysed"
)
NOT_WARNING = re.compile(
    r"^(Disk |Sector size|Partition table holds|Main partition table begins|"
    r"First usable|Partitions will be|Total free|Number |No problems|Units:|Device )"
)
SGDISK_ROW = re.compile(r"^\s+(\d+)\s+(\d+)\s+(\d+)\s+\S+\s+\S+\s+(\S+)\s")
FDISK_ROW = re.compile(r"^\S+img\d+\s+(\d+)\s+(\d+)\s+\d+\s+\S+\s+(.*)$")
PARTX_ROW = re.compile(r"^\s*\d+\s+(\d+)\s+(\d+)\s+\d+\s")
MMLS_ROW = re.compile(r"^\d{3}:\s+(\S+)\s+(\d+)\s+(\d+)\s+(\d+)\s*(.*)$")
MMLS_PART_SLOT = re.compile(r"^\d{3}(:\d{3})?$")


def run(argv):
    try:
        proc = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                              timeout=TIMEOUT, env=ENV, check=False)
        return (proc.stdout.decode("utf-8", "replace"),
                proc.stderr.decode("utf-8", "replace"), str(proc.returncode))
    except subprocess.TimeoutExpired as e:
        return ((e.stdout or b"").decode("utf-8", "replace"),
                (e.stderr or b"").decode("utf-8", "replace"),
                f"TIMEOUT after {TIMEOUT}s")


def partitions(tool, stdout):
    found = []
    for line in stdout:
        if tool == "sgdisk-p":
            m = SGDISK_ROW.match(line)
            if m:
                found.append(f"{m.group(2)}-{m.group(3)}, type {m.group(4)} (sgdisk code)")
        elif tool == "fdisk-l":
            m = FDISK_ROW.match(line)
            if m:
                found.append(f"{m.group(1)}-{m.group(2)}, type {m.group(3).strip()}")
        elif tool == "partx":
            m = PARTX_ROW.match(line)
            if m:
                found.append(f"{m.group(1)}-{m.group(2)}, type not shown")
        elif tool == "mmls":
            m = MMLS_ROW.match(line)
            if m and MMLS_PART_SLOT.match(m.group(1)):
                desc = m.group(5).strip() or "no description"
                found.append(f"{int(m.group(2))}-{int(m.group(3))}, "
                             f"type not shown (mmls: {desc})")
    return found


def paragraphs(lines):
    """Group lines into paragraphs that start at a PARA_START line, verbatim."""
    out, current = [], None
    for line in lines:
        if not line.strip() or NOT_WARNING.match(line):
            if current is not None:
                out.append(current)
                current = None
            continue
        if PARA_START.match(line):
            if current is not None:
                out.append(current)
            current = [line]
        elif current is not None:
            current.append(line)
    if current is not None:
        out.append(current)
    return [" / ".join(p) for p in out]


def warnings(tool, stdout, stderr):
    if tool == "blkid":
        return []
    err = [line for line in stderr if line.strip()]
    return paragraphs(stdout) + ([" / ".join(err)] if err else [])


def summary_row(case_id, tool, stdout, stderr, status):
    if tool == "blkid":
        pt = [line for line in stdout if line.startswith("PTTYPE=")]
        parts_cell = ("none (blkid -p lists no partitions for a whole disk; "
                      + (pt[0] if pt else "no PTTYPE") + ")")
    else:
        parts = partitions(tool, stdout)
        parts_cell = "; ".join(parts) if parts else "none"
    warns = warnings(tool, stdout, stderr)
    warn_cell = " | ".join(warns) if warns else "-"
    return "\t".join([case_id, tool, status, parts_cell, warn_cell])


def tool_versions():
    lines = []
    for name, argv in VERSION_COMMANDS.items():
        stdout, stderr, status = run(argv)
        first = next((l for l in (stdout + stderr).split("\n") if l.strip()), "")
        lines.append(f"{name}: {first.strip()}  (exit status {status})")
    return lines


def main(argv):
    if len(argv) != 3:
        sys.exit(__doc__)
    casedir, outdir = argv[1], argv[2]
    if os.path.exists(outdir):
        raise SystemExit(f"STOP: {outdir} exists")
    for case_id in CASE_IDS:
        if not os.path.isfile(os.path.join(casedir, f"{case_id}.img")):
            raise SystemExit(f"STOP: {case_id}.img missing in {casedir}")
    os.mkdir(outdir)
    outs = os.path.join(outdir, "out")
    os.mkdir(outs)
    with open(os.path.join(outdir, "versions.txt"), "x", encoding="utf-8") as w:
        w.write("\n".join(tool_versions()) + "\n")
    rows = ["case\ttool\texit status\tpartitions reported\t"
            "GPT warning text (copied exactly; wrapped lines joined with ' / ')"]
    files = 0
    for case_id in CASE_IDS:
        img = os.path.join(casedir, f"{case_id}.img")
        for tool, argv_tool in TOOLS.items():
            full = argv_tool + [img]
            stdout_text, stderr_text, status = run(full)
            dest = os.path.join(outs, f"{case_id}.{tool}.txt")
            with open(dest, "x", encoding="utf-8") as w:
                w.write(f"command: {' '.join(full)}\n")
                w.write(f"stdout:\n{stdout_text}")
                w.write(f"\nstderr:\n{stderr_text}")
                w.write(f"\nexit status: {status}\n")
            files += 1
            rows.append(summary_row(case_id, tool, stdout_text.split("\n"),
                                    stderr_text.split("\n"), status))
        print(f"ran {case_id}")
    with open(os.path.join(outdir, "summary.tsv"), "x", encoding="utf-8") as w:
        w.write("\n".join(rows) + "\n")
    print(f"files in out/: {files}")
    print(f"rows in summary.tsv (excluding header): {len(rows) - 1}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
