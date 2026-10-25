#include <errno.h>
#include <stdlib.h>
#include <unistd.h>

/* fchdir then exec. The daemon is forbidden to use unsafe, so it cannot
   fchdir in a pre_exec hook. This helper is the child image for one syscall. */
int main(int argc, char **argv) {
    char *end = NULL;
    long fd;
    if (argc < 3) {
        return 127;
    }
    errno = 0;
    fd = strtol(argv[1], &end, 10);
    if (end == argv[1] || *end != '\0' || errno != 0 || fd < 0 || fd > 2147483647) {
        return 127;
    }
    if (fchdir((int)fd) != 0) {
        return 127;
    }
    execvp(argv[2], &argv[2]);
    return 127;
}
