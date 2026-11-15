// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

extern long timer_syscall(long number, const long *arguments);

#define P(value) ((long)(value))
#define SC(number, ...) timer_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) \
    do { \
        if (!(condition)) { \
            timer_fail(__LINE__); \
        } \
    } while (0)

struct timeval {
    long seconds;
    long microseconds;
};

struct itimerval {
    struct timeval interval;
    struct timeval value;
};

struct timespec {
    long seconds;
    long nanoseconds;
};

struct sigaction {
    void (*handler)(int);
    unsigned long flags;
    unsigned long mask;
};

static volatile unsigned real_count;
static volatile unsigned virtual_count;
static volatile unsigned prof_count;

static void timer_fail(unsigned line) {
    char text[] = "Timer assertion line 0000\n";
    for (int index = 25; index >= 22; --index) {
        text[index] = (char)('0' + line % 10);
        line /= 10;
    }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 101);
    for (;;) {
    }
}

static void real_handler(int number) {
    if (number == 14) {
        ++real_count;
    }
}

static void virtual_handler(int number) {
    if (number == 26) {
        ++virtual_count;
    }
}

static void prof_handler(int number) {
    if (number == 27) {
        ++prof_count;
    }
}

static void install(int number, void (*handler)(int)) {
    struct sigaction action;
    action.handler = handler;
    action.flags = 0;
    action.mask = 0;
    CHECK(SC(134, number, P(&action), 0, 8) == 0);
}

static long monotonic_ns(void) {
    struct timespec now;
    CHECK(SC(113, 1, P(&now)) == 0);
    return now.seconds * 1000000000L + now.nanoseconds;
}

static void set_timer(int which, long interval_seconds, long interval_microseconds,
                      long value_seconds, long value_microseconds) {
    struct itimerval timer;
    timer.interval.seconds = interval_seconds;
    timer.interval.microseconds = interval_microseconds;
    timer.value.seconds = value_seconds;
    timer.value.microseconds = value_microseconds;
    CHECK(SC(103, which, P(&timer), 0) == 0);
}

static void get_timer(int which, struct itimerval *timer) {
    CHECK(SC(102, which, P(timer)) == 0);
}

static int zero(struct timeval *value) {
    return value->seconds == 0 && value->microseconds == 0;
}

static int within(struct timeval *value, long seconds, long microseconds) {
    if (value->seconds != seconds) {
        return value->seconds < seconds;
    }
    return value->microseconds <= microseconds;
}

static void stop_timer(int which, long interval_seconds, long interval_microseconds) {
    set_timer(which, interval_seconds, interval_microseconds, 0, 0);
    struct itimerval timer;
    get_timer(which, &timer);
    CHECK(zero(&timer.interval) && zero(&timer.value));
}

static void sleep_us(long microseconds) {
    struct timespec duration;
    duration.seconds = 0;
    duration.nanoseconds = microseconds * 1000;
    long result = SC(101, P(&duration), 0);
    CHECK(result == 0 || result == -4);
}

static void wait_real(unsigned target) {
    long deadline = monotonic_ns() + 2000000000L;
    while (real_count < target) {
        CHECK(monotonic_ns() < deadline);
    }
}

static void wait_cpu(volatile unsigned *count, unsigned target) {
    long deadline = monotonic_ns() + 3000000000L;
    while (*count < target) {
        for (volatile unsigned spin = 0; spin < 1000000; ++spin) {
        }
        CHECK(monotonic_ns() < deadline);
    }
}

static void wait_child(long child) {
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child);
    CHECK(status == 42 << 8);
}

void timer_selftest(void) {
    struct itimerval timer;
    unsigned long mask = 1UL << (14 - 1);
    unsigned long saved_mask;
    install(14, real_handler);
    install(26, virtual_handler);
    install(27, prof_handler);

    CHECK(SC(102, 3, P(&timer)) == -22);
    CHECK(SC(102, -1, P(&timer)) == -22);
    CHECK(SC(102, 0, 0) == -14);
    CHECK(SC(103, 3, P(&timer), 0) == -22);
    CHECK(SC(103, 0, 1, 0) == -14);
    struct itimerval negative_interval = {{-1, 0}, {0, 0}};
    CHECK(SC(103, 0, P(&negative_interval), 0) == -22);
    struct itimerval negative_value = {{0, 0}, {0, -1}};
    CHECK(SC(103, 0, P(&negative_value), 0) == -22);
    struct itimerval bad_microseconds = {{0, 1000000}, {1, 0}};
    CHECK(SC(103, 0, P(&bad_microseconds), 0) == -22);
    CHECK(SC(103, 0, 0, 0) == 0);

    set_timer(0, 0, 0, 0, 100000);
    get_timer(0, &timer);
    CHECK(within(&timer.value, 0, 100000) && timer.value.seconds >= 0);
    CHECK(zero(&timer.interval));
    wait_real(1);
    CHECK(real_count == 1);
    get_timer(0, &timer);
    CHECK(zero(&timer.value));
    // Consume periodic expiries synchronously so handler overhead cannot starve the test.
    CHECK(SC(135, 0, P(&mask), P(&saved_mask), 8) == 0);
    set_timer(0, 0, 20000, 0, 20000);
    get_timer(0, &timer);
    CHECK(timer.interval.seconds == 0 && timer.interval.microseconds == 20000);
    CHECK(timer.value.seconds == 0 && timer.value.microseconds <= 20000);
    struct timespec timeout = {2, 0};
    for (unsigned expiry = 0; expiry < 3; ++expiry) {
        CHECK(SC(137, P(&mask), 0, P(&timeout), 8) == 14);
    }
    get_timer(0, &timer);
    CHECK(within(&timer.value, 0, 20000));
    CHECK(timer.interval.seconds == 0 && timer.interval.microseconds == 20000);
    stop_timer(0, 0, 0);
    CHECK(SC(135, 2, P(&saved_mask), 0, 8) == 0);
    sleep_us(1000);
    unsigned stopped = real_count;
    sleep_us(60000);
    CHECK(real_count == stopped);

    set_timer(1, 0, 0, 0, 100000);
    sleep_us(150000);
    CHECK(virtual_count == 0);
    wait_cpu(&virtual_count, 1);
    get_timer(1, &timer);
    CHECK(zero(&timer.value));

    set_timer(1, 0, 20000, 0, 20000);
    wait_cpu(&virtual_count, 3);
    stop_timer(1, 0, 0);

    set_timer(2, 0, 0, 0, 50000);
    wait_cpu(&prof_count, 1);
    get_timer(2, &timer);
    CHECK(zero(&timer.value));

    // Keep expiry from interrupting the fork and exec lifecycle checks.
    CHECK(SC(135, 0, P(&mask), P(&saved_mask), 8) == 0);
    set_timer(0, 1, 0, 1, 0);
    long child = SC(220, 17, 0, 0, 0, 0);
    if (child == 0) {
        get_timer(0, &timer);
        CHECK(zero(&timer.value) && zero(&timer.interval));
        SC(94, 42);
        for (;;) {
        }
    }
    CHECK(child > 0);
    wait_child(child);
    get_timer(0, &timer);
    CHECK(timer.interval.seconds == 1 && timer.interval.microseconds == 0);
    CHECK(within(&timer.value, 1, 0));
    stop_timer(0, 0, 0);
    sleep_us(20000);

    unsigned before_exec = real_count;
    child = SC(220, 17, 0, 0, 0, 0);
    if (child == 0) {
        set_timer(0, 1, 0, 1, 0);
        static char path[] = "/bin/linux-test";
        static char mode[] = "timer-exec";
        static char environment[] = "ARONA_TEST=yes";
        static char *arguments[] = {path, mode, 0};
        static char *variables[] = {environment, 0};
        SC(221, P(path), P(arguments), P(variables));
        SC(94, 1);
        for (;;) {
        }
    }
    CHECK(child > 0);
    wait_child(child);
    CHECK(real_count == before_exec);
    CHECK(SC(135, 2, P(&saved_mask), 0, 8) == 0);
}

void timer_exec_check(void) {
    struct itimerval timer;
    get_timer(0, &timer);
    CHECK(timer.interval.seconds == 1 && timer.interval.microseconds == 0);
    CHECK(within(&timer.value, 1, 0));
    SC(94, 42);
    for (;;) {
    }
}
