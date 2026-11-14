#!/usr/bin/env bash
# Usage: exp-0011-deleted-dirs-images.sh WORKDIR
#
# Makes the volumes of ADR-0022 Appendix D with FatFs R0.16 and scores each
# against the tags its writer put in every block, with deleted directories
# read (exp-0011-scan.py --deleted-dirs) and not read. Input, not committed,
# in WORKDIR/src: ff16.zip (https://elm-chan.org/fsw/ff/arc/ff16.zip), the
# archive exp-0011-made-images.sh names, checked by digest first. FatFs is
# built with its clock fixed at 2026-03-01 10:00:00, two seconds per call,
# so each volume is the same on every run; their digests are printed.
# Between a volume's two FatFs phases, exp-0011-mac-delete.py deletes a
# directory as macOS leaves one: its own set marked deleted, its whole tree
# freed in the bitmap, the entries inside left in use. A failure prints a
# line starting FAIL; an archive that does not match its digest, or a FatFs
# configuration that did not take, stops the script with nothing built.
set -u
W=${1:-}
[ -n "$W" ] || { echo "usage: exp-0011-deleted-dirs-images.sh WORKDIR" >&2; exit 2; }
T=$(cd "$(dirname "$0")" && pwd)
S=$W/src
B=$W/deleted-dirs

want=99f7dc1f7e095356e4a9e3dbe29959090d8b948afe2bbc5441e52fdf4b85449e
got=$(sha256sum "$S/ff16.zip" 2>/dev/null | cut -c1-64)
if [ "$got" != "$want" ]; then
    echo "FAIL ff16.zip is ${got:-missing}, expected $want: nothing is built or run"
    exit 1
fi

rm -rf -- "${B:?}"
mkdir -p "$B/bin" "$B/img" "$B/fatfs"
python3 -I -m zipfile -e "$S/ff16.zip" "$B/fatfs" || echo "FAIL unpack FatFs"
F=$B/fatfs/source
sed -i -e 's/^#define FF_USE_MKFS\t\t0/#define FF_USE_MKFS\t\t1/' \
    -e 's/^#define FF_CODE_PAGE\t932/#define FF_CODE_PAGE\t437/' \
    -e 's/^#define FF_USE_LFN\t\t0/#define FF_USE_LFN\t\t1/' \
    -e 's/^#define FF_FS_EXFAT\t\t0/#define FF_FS_EXFAT\t\t1/' "$F/ffconf.h"
[ "$(grep -c -e $'^#define FF_USE_MKFS\t\t1' -e $'^#define FF_CODE_PAGE\t437' \
    -e $'^#define FF_USE_LFN\t\t1' -e $'^#define FF_FS_EXFAT\t\t1' \
    -e $'^#define FF_FS_NORTC\t\t0' "$F/ffconf.h")" = 5 ] ||
    { echo "FAIL configure FatFs: nothing more is built or run"; exit 1; }
cat >"$F/host_s5.c" <<'HOST_S5_END'
/* Builds the stage-5 benchmark volumes with FatFs R0.16, its clock fixed
   (2026-03-01 10:00:00 plus two seconds per call), so each volume is the
   same on every run. The volume is partitioned (MBR), not a superfloppy.
   Every 512-byte block of every file holds "TAG <path> <block>" padded
   with '.', so which file wrote a cluster is read from the cluster.

   Usage: host_s5 macos|reused build|after IMAGE SIZE_MB CLUSTER_BYTES

   macos build: y/ and D/ made; y/Y.bin and y/X.bin written and deleted;
       remount (FatFs then allocates from the heap's start, so Y's and X's
       clusters are reused); D/A.bin, D/S/ and D/S/B.bin written into them;
       T.bin live. Then D is deleted as macOS deletes a directory
       (mac_delete.py): its own set marked deleted, every cluster of its
       tree freed in the bitmap, the entries inside left in use.
   macos after: nothing written.
   reused build: l/ and E/ made, E/C.bin written; U.bin live. Then
       mac_delete.py E.
   reused after: remount; l/L.bin written live: it takes E's freed
       directory cluster, so E's cluster is in use again; l/ keeps L's set
       out of the root, where E's deleted set stays. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "ff.h"
#include "diskio.h"

static FILE *img;
static LBA_t sectors;
static DWORD ticks;

DWORD get_fattime(void)
{
	DWORD s = 10 * 3600 + 2 * ticks++;
	DWORD h = s / 3600, m = (s / 60) % 60, ds = (s % 60) / 2;
	return ((DWORD)(2026 - 1980) << 25) | (3u << 21) | (1u << 16) | (h << 11) | (m << 5) | ds;
}
DSTATUS disk_initialize(BYTE pdrv) { (void)pdrv; return 0; }
DSTATUS disk_status(BYTE pdrv) { (void)pdrv; return 0; }
DRESULT disk_read(BYTE pdrv, BYTE *buff, LBA_t sector, UINT count)
{
	(void)pdrv;
	if (fseek(img, (long)sector * 512, SEEK_SET) || fread(buff, 512, count, img) != count)
		return RES_ERROR;
	return RES_OK;
}
DRESULT disk_write(BYTE pdrv, const BYTE *buff, LBA_t sector, UINT count)
{
	(void)pdrv;
	if (fseek(img, (long)sector * 512, SEEK_SET) || fwrite(buff, 512, count, img) != count)
		return RES_ERROR;
	return RES_OK;
}
DRESULT disk_ioctl(BYTE pdrv, BYTE cmd, void *buff)
{
	(void)pdrv;
	switch (cmd) {
	case CTRL_SYNC: fflush(img); return RES_OK;
	case GET_SECTOR_COUNT: *(LBA_t *)buff = sectors; return RES_OK;
	case GET_SECTOR_SIZE: *(WORD *)buff = 512; return RES_OK;
	case GET_BLOCK_SIZE: *(DWORD *)buff = 1; return RES_OK;
	}
	return RES_PARERR;
}
static void die(const char *what, const char *path, FRESULT r)
{
	fprintf(stderr, "%s %s: %d\n", what, path, (int)r);
	exit(1);
}
static void tagged(const char *path, unsigned blocks)
{
	FIL f;
	UINT done;
	char buf[512];
	FRESULT r = f_open(&f, path, FA_WRITE | FA_CREATE_ALWAYS);
	if (r) die("open", path, r);
	for (unsigned b = 0; b < blocks; b++) {
		memset(buf, '.', sizeof buf);
		int n = snprintf(buf, sizeof buf, "TAG %s %u ", path, b);
		if (n < 0 || n >= (int)sizeof buf) die("tag", path, FR_INVALID_PARAMETER);
		buf[n] = '.';
		if ((r = f_write(&f, buf, sizeof buf, &done)) || done != sizeof buf) die("write", path, r);
	}
	if ((r = f_close(&f))) die("close", path, r);
	printf("WROTE\t%s\t%u\n", path, blocks);
}
static void gone(const char *path)
{
	FRESULT r = f_unlink(path);
	if (r) die("unlink", path, r);
	printf("DELETED\t%s\n", path);
}
static void dir(const char *path)
{
	FRESULT r = f_mkdir(path);
	if (r) die("mkdir", path, r);
	printf("MKDIR\t%s\n", path);
}
static void remount(FATFS *fs)
{
	FRESULT r;
	f_unmount("");
	if ((r = f_mount(fs, "", 1))) die("mount", "", r);
}

int main(int argc, char **argv)
{
	static BYTE work[FF_MAX_SS * 64];
	FATFS fs;
	FRESULT r;
	if (argc != 6) {
		fprintf(stderr, "usage: host_s5 macos|reused build|after IMAGE SIZE_MB CLUSTER_BYTES\n");
		return 2;
	}
	const char *scenario = argv[1], *phase = argv[2];
	DWORD cluster = (DWORD)strtoul(argv[5], 0, 10);
	unsigned per = cluster / 512;
	sectors = (LBA_t)strtoul(argv[4], 0, 10) * 2048;
	/* Each phase begins where the last left the clock. */
	ticks = !strcmp(phase, "after") ? 1000 : 0;

	if (!strcmp(phase, "build")) {
		MKFS_PARM opt = { FM_EXFAT, 1, 0, 0, cluster };
		img = fopen(argv[3], "w+b");
		if (!img || fseek(img, (long)sectors * 512 - 1, SEEK_SET) || fputc(0, img) == EOF) return 1;
		if ((r = f_mkfs("", &opt, work, sizeof work))) die("mkfs", argv[3], r);
	} else {
		img = fopen(argv[3], "r+b");
		if (!img) return 1;
	}
	if ((r = f_mount(&fs, "", 1))) die("mount", argv[3], r);

	if (!strcmp(scenario, "macos") && !strcmp(phase, "build")) {
		/* y/ first, so the files deleted in it keep their sets; D's own
		   cluster is taken before Y and X are written, so only D's files
		   reuse their clusters. */
		dir("y");
		dir("D");
		tagged("y/Y.bin", 3 * per);
		tagged("y/X.bin", 2 * per);
		gone("y/Y.bin");
		gone("y/X.bin");
		remount(&fs);
		tagged("D/A.bin", 3 * per);
		dir("D/S");
		tagged("D/S/B.bin", 2 * per);
		tagged("T.bin", per);
	} else if (!strcmp(scenario, "macos") && !strcmp(phase, "after")) {
		/* Nothing written: the volume is examined as macOS left it. */
	} else if (!strcmp(scenario, "reused") && !strcmp(phase, "build")) {
		dir("l");
		dir("E");
		tagged("E/C.bin", 2 * per);
		tagged("U.bin", per);
	} else if (!strcmp(scenario, "reused") && !strcmp(phase, "after")) {
		tagged("l/L.bin", per);
	} else {
		fprintf(stderr, "unknown scenario or phase\n");
		return 2;
	}
	f_unmount("");
	fclose(img);
	return 0;
}
HOST_S5_END
(cd "$F" && gcc -O1 -o "$B/bin/fatfs-deleted-dirs" host_s5.c ff.c ffsystem.c ffunicode.c) ||
    echo "FAIL build FatFs host"

I=$B/img
for s in macos reused; do
    d=$([ "$s" = macos ] && echo D || echo E)
    for c in 4096 512; do
        f=$I/s5-$s-$c.img
        { "$B/bin/fatfs-deleted-dirs" "$s" build "$f" 8 "$c" &&
          python3 -I "$T/exp-0011-mac-delete.py" "$f" "$d" &&
          "$B/bin/fatfs-deleted-dirs" "$s" after "$f" 8 "$c"; } >/dev/null ||
            echo "FAIL make $(basename "$f")"
    done
done
(cd "$I" && sha256sum s5-*.img)

for f in "$I"/*.img; do
    b=$(basename "$f" .img)
    for scope in read unread; do
        flag=$([ "$scope" = read ] && echo --deleted-dirs)
        python3 -I "$T/exp-0011-scan.py" "$f" $flag >"$B/$b.$scope.tsv" || echo "FAIL scan $b $scope"
        grep -h '^# deleted_dir_unread' "$B/$b.$scope.tsv" | sed "s/^/$b $scope /"
        python3 -I "$T/exp-0011-score-tags.py" "$f" "$B/$b.$scope.tsv" >"$B/$b.$scope.score" ||
            echo "FAIL score $b $scope"
        grep '^TALLY' "$B/$b.$scope.score" | grep -w decided | sed "s/^/$scope /"
        python3 -I "$T/exp-0011-condition-two.py" "$f" "$B/$b.$scope.tsv" >"$B/$b.$scope.c2" ||
            echo "FAIL condition two $b $scope"
        grep -e '^SET' -e '^TALLY' "$B/$b.$scope.c2" | sed "s/^/$scope /"
    done
done
