#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/signalfd.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int failed(const char *operation)
{
    perror(operation);
    return EXIT_FAILURE;
}

static int reap(pid_t child)
{
    int status;
    pid_t result;
    do {
        result = waitpid(child, &status, 0);
    } while (result < 0 && errno == EINTR);
    if (result < 0)
        return failed("waitpid");
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

int main(int argc, char **argv)
{
    if (argc < 4 || strspn(argv[1], "0123456789") != strlen(argv[1])) {
        fprintf(stderr, "Usage: pidfd-run WATCHDOG_MS SCRATCH COMMAND [ARGS...]\n");
        return EXIT_FAILURE;
    }
    errno = 0;
    const long milliseconds = strtol(argv[1], NULL, 10);
    if (errno || milliseconds <= 0) {
        fprintf(stderr, "WATCHDOG_MS must be a positive integer\n");
        return EXIT_FAILURE;
    }

    sigset_t signals, previous;
    if (sigemptyset(&signals) < 0 ||
        sigaddset(&signals, SIGINT) < 0 ||
        sigaddset(&signals, SIGTERM) < 0 ||
        sigaddset(&signals, SIGHUP) < 0 ||
        sigprocmask(SIG_BLOCK, &signals, &previous) < 0)
        return failed("block supervisor signals");
    const int notice = signalfd(-1, &signals, SFD_CLOEXEC);
    const int timer = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    if (notice < 0 || timer < 0)
        return failed("open supervisor event descriptors");
    const struct itimerspec deadline = {
        .it_value = {.tv_sec = milliseconds / 1000, .tv_nsec = (milliseconds % 1000) * 1000000}
    };
    if (timerfd_settime(timer, 0, &deadline, NULL) < 0)
        return failed("arm launcher watchdog");

    int start[2];
    if (pipe2(start, O_CLOEXEC) < 0)
        return failed("pipe2");
    const pid_t child = fork();
    if (child < 0)
        return failed("fork");
    if (child == 0) {
        close(start[1]);
        char token;
        if (read(start[0], &token, 1) != 1)
            _exit(EXIT_FAILURE);
        if (sigprocmask(SIG_SETMASK, &previous, NULL) == 0)
            execvp(argv[3], &argv[3]);
        _exit(failed("exec isolated launcher"));
    }
    close(start[0]);
    // The child cannot exec until the parent holds its pidfd. It has not
    // been reaped, so pidfd_open cannot attach to a reused numeric PID.
    const int pidfd = (int)syscall(SYS_pidfd_open, child, 0);
    if (pidfd < 0 || write(start[1], "+", 1) != 1) {
        failed("start isolated launcher with pidfd");
        close(start[1]);
        (void)reap(child);
        return EXIT_FAILURE;
    }
    close(start[1]);

    struct pollfd watched[] = {
        {.fd = pidfd, .events = POLLIN},
        {.fd = timer, .events = POLLIN},
        {.fd = notice, .events = POLLIN},
    };
    int observed;
    do {
        observed = poll(watched, sizeof watched / sizeof *watched, -1);
    } while (observed < 0 && errno == EINTR);
    if (observed > 0 && watched[0].revents & POLLIN) {
        const int status = reap(child);
        printf("Isolated process namespace exited (%d)\n", status);
        if (status != 0)
            return status;
        if (fflush(stdout) == EOF)
            return failed("flush launcher status");
        execlp("rm", "rm", "-rf", "--one-file-system", "--", argv[2], (char *)NULL);
        return failed("prune launcher scratch");
    }
    int status = EXIT_FAILURE;
    if (observed < 0) {
        failed("poll supervisor");
    } else if (watched[1].revents & POLLIN) {
        fprintf(stderr, "Isolated launcher reached its watchdog deadline\n");
        status = 124;
    } else if (watched[2].revents & POLLIN) {
        struct signalfd_siginfo signal;
        if (read(notice, &signal, sizeof signal) == (ssize_t)sizeof signal)
            status = 128 + (int)signal.ssi_signo;
        else
            failed("read supervisor signal");
    }
    if (syscall(SYS_pidfd_send_signal, pidfd, SIGKILL, NULL, 0) < 0 && errno != ESRCH)
        return failed("stop isolated launcher by pidfd");
    (void)reap(child);
    return status;
}
