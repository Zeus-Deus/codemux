/* Diagnostic only: emulate a 200 Hz vblank clock on EOPNOTSUPP.
 * Does not synchronize with real scanout and must not be shipped.
 * Only blocking DRM_VBLANK_RELATIVE requests for sequence 0 or 1 qualify.
 */
#define _GNU_SOURCE
#include <xf86drm.h>
#include <dlfcn.h>
#include <errno.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

typedef int (*wait_fn)(int, drmVBlankPtr);
static wait_fn real_wait;
static pthread_once_t lookup_once = PTHREAD_ONCE_INIT;
static pthread_once_t announce_once = PTHREAD_ONCE_INIT;
static const uint64_t period_ns = UINT64_C(5000000);

static void lookup_real(void)
{
    *(void **)(&real_wait) = dlsym(RTLD_NEXT, "drmWaitVBlank");
}
static void announce(void)
{
    fputs("[diagnostic-vblank] Emulating 200 Hz for unsupported DRM vblank; not scanout synchronized.\n", stderr);
}
static uint64_t time_ns(const struct timespec *ts)
{
    return (uint64_t)ts->tv_sec * UINT64_C(1000000000) + (uint64_t)ts->tv_nsec;
}

int drmWaitVBlank(int fd, drmVBlankPtr vbl)
{
    pthread_once(&lookup_once, lookup_real);
    if (!real_wait) {
        errno = ENOSYS;
        return -ENOSYS;
    }
    if (!vbl)
        return real_wait(fd, vbl);

    const drmVBlankReq request = vbl->request;
    int result = real_wait(fd, vbl);
    const int real_errno = errno;
    const unsigned type = (unsigned)request.type;
    const unsigned allowed = DRM_VBLANK_RELATIVE | DRM_VBLANK_HIGH_CRTC_MASK | DRM_VBLANK_SECONDARY;
    if (!result || real_errno != EOPNOTSUPP ||
        (type & ~allowed) || !(type & DRM_VBLANK_RELATIVE) ||
        request.sequence > 1 || request.signal) {
        errno = real_errno;
        return result;
    }

    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0)
        return result;
    if (request.sequence == 1) {
        uint64_t deadline_ns = (time_ns(&now) / period_ns + 1) * period_ns;
        struct timespec deadline = {
            .tv_sec = (time_t)(deadline_ns / UINT64_C(1000000000)),
            .tv_nsec = (long)(deadline_ns % UINT64_C(1000000000)),
        };
        int sleep_error;
        do {
            sleep_error = clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &deadline, NULL);
        } while (sleep_error == EINTR);
        if (sleep_error) {
            errno = sleep_error;
            return -sleep_error;
        }
        if (clock_gettime(CLOCK_MONOTONIC, &now) != 0)
            return result;
    }
    pthread_once(&announce_once, announce);
    vbl->reply.type = request.type;
    vbl->reply.sequence = (unsigned)(time_ns(&now) / period_ns);
    vbl->reply.tval_sec = now.tv_sec;
    vbl->reply.tval_usec = now.tv_nsec / 1000;
    errno = 0;
    return 0;
}
