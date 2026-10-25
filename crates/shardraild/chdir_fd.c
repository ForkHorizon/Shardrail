#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* Open the admitted path, check it is still the pinned directory, fchdir,
   and exec. The daemon cannot fchdir: unsafe_code is forbidden. The helper
   opens the path itself so no directory fd is inherited across the spawn. */
static int fail(const char *message) {
    fprintf(stderr, "chdir-fd: %s\n", message);
    return 1;
}

static int parse_u64(const char *text, unsigned long long *out) {
    char *end = NULL;
    if (text == NULL || text[0] == '\0') {
        return -1;
    }
    errno = 0;
    *out = strtoull(text, &end, 10);
    if (end == text || *end != '\0' || errno != 0) {
        return -1;
    }
    return 0;
}

int main(int argc, char **argv) {
    unsigned long long dev = 0;
    unsigned long long ino = 0;
    struct stat st;
    int fd;
    if (argc < 5) {
        return fail("usage: path dev ino program [args...]");
    }
    if (parse_u64(argv[2], &dev) != 0) {
        return fail("device is invalid");
    }
    if (parse_u64(argv[3], &ino) != 0) {
        return fail("inode is invalid");
    }
    fd = open(argv[1], O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) {
        return fail("cannot open directory");
    }
    if (fstat(fd, &st) != 0) {
        close(fd);
        return fail("cannot stat directory");
    }
    if ((unsigned long long)st.st_dev != dev || (unsigned long long)st.st_ino != ino) {
        close(fd);
        return fail("directory identity does not match the pinned workdir");
    }
    if (fchdir(fd) != 0) {
        close(fd);
        return fail("cannot enter directory");
    }
    /* Close before exec so the child does not inherit this directory fd.
       O_CLOEXEC is the backup if exec races with the close. */
    if (close(fd) != 0) {
        return fail("cannot close directory");
    }
    execvp(argv[4], &argv[4]);
    return fail("exec failed");
}
