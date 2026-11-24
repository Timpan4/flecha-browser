#define _POSIX_C_SOURCE 200809L
#include <X11/Xlib.h>
#include <X11/extensions/XTest.h>
#include <X11/keysym.h>
#include <limits.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static int coordinate(const char *text)
{
    char *end;
    long number = strtol(text, &end, 10);
    return end == text || *end != '\0' || number < 0 || number > INT_MAX ? -1 : (int)number;
}

static int press(Display *display, KeySym symbol, Bool down)
{
    KeyCode code = XKeysymToKeycode(display, symbol);
    return code != 0 && XTestFakeKeyEvent(display, code, down, CurrentTime);
}

static int tap(Display *display, KeySym symbol)
{
    return press(display, symbol, True) && press(display, symbol, False);
}

static int click(Display *display, int x, int y, unsigned int button)
{
    Window root = DefaultRootWindow(display);
    Window child, ignored_window;
    int ignored_coord;
    unsigned int ignored_mask;
    if (!XTestFakeMotionEvent(display, DefaultScreen(display), x, y, CurrentTime) ||
        !XQueryPointer(display, root, &ignored_window, &child, &ignored_coord,
                       &ignored_coord, &ignored_coord, &ignored_coord, &ignored_mask) ||
        child == None)
        return 0;
    XSetInputFocus(display, child, RevertToParent, CurrentTime);
    return XTestFakeButtonEvent(display, button, True, CurrentTime) &&
           XTestFakeButtonEvent(display, button, False, CurrentTime);
}

// The one viewable top-level window named `title`, or None.
static Window named(Display *display, const char *title)
{
    Window root = DefaultRootWindow(display);
    Window returned_root, parent, *children;
    unsigned int count;
    if (!XQueryTree(display, root, &returned_root, &parent, &children, &count))
        return None;

    Window match = None;
    for (unsigned int i = 0; i < count; i++) {
        char *name = NULL;
        XWindowAttributes attributes;
        if (XFetchName(display, children[i], &name) && name != NULL &&
            strcmp(name, title) == 0 && XGetWindowAttributes(display, children[i], &attributes) &&
            attributes.map_state == IsViewable) {
            if (match != None) {
                XFree(name);
                XFree(children);
                return None;
            }
            match = children[i];
        }
        if (name != NULL)
            XFree(name);
    }
    if (children != NULL)
        XFree(children);
    return match;
}

static int geometry(Display *display, const char *title)
{
    Window root = DefaultRootWindow(display);
    Window match = named(display, title);
    if (match == None)
        return 0;

    XWindowAttributes attributes;
    int x, y;
    Window child;
    if (!XGetWindowAttributes(display, match, &attributes) ||
        !XTranslateCoordinates(display, match, root, 0, 0, &x, &y, &child))
        return 0;
    return printf("%d %d %d %d\n", x, y, attributes.width, attributes.height) > 0;
}

static int64_t milliseconds(void)
{
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0)
        return -1;
    return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static int paste(Display *display)
{
    // Match the pinned GPUI X11 clipboard's selection deadline and poll cadence.
    int64_t start = milliseconds();
    if (start < 0)
        return 0;
    const int64_t deadline = start + 4000;
    Atom clipboard = XInternAtom(display, "CLIPBOARD", False);
    while (XGetSelectionOwner(display, clipboard) == None) {
        int64_t now = milliseconds();
        if (now < 0 || now >= deadline) {
            fprintf(stderr, "No X11 CLIPBOARD owner appeared\n");
            return 0;
        }
        nanosleep(&(struct timespec){.tv_nsec = 1000000}, NULL);
    }
    Atom utf8 = XInternAtom(display, "UTF8_STRING", False);
    Atom property = XInternAtom(display, "FLECHA_SPIKE", False);
    Window requestor = XCreateSimpleWindow(display, DefaultRootWindow(display),
                                          0, 0, 1, 1, 0, 0, 0);
    XConvertSelection(display, clipboard, utf8, property, requestor, CurrentTime);
    XFlush(display);
    XEvent event;
    while (!XCheckTypedWindowEvent(display, requestor, SelectionNotify, &event)) {
        int64_t now = milliseconds();
        struct pollfd connection = {.fd = ConnectionNumber(display), .events = POLLIN};
        if (now < 0 || now >= deadline || poll(&connection, 1, (int)(deadline - now)) <= 0) {
            fprintf(stderr, "X11 CLIPBOARD owner did not send SelectionNotify\n");
            return 0;
        }
    }
    if (event.xselection.property != property)
        return 0;
    Atom type;
    int format;
    unsigned long length, remaining;
    unsigned char *text = NULL;
    int status = XGetWindowProperty(display, requestor, property, 0, 1, True,
                                    utf8, &type, &format, &length, &remaining, &text);
    int ok = status == Success && type == utf8 && format == 8 && length == 1 &&
             remaining == 0 && text != NULL && text[0] == 'x';
    if (text != NULL)
        XFree(text);
    return ok;
}

int main(int argc, char **argv)
{
    if (argc < 2)
        return EXIT_FAILURE;
    Display *display = XOpenDisplay(NULL);
    if (display == NULL)
        return EXIT_FAILURE;
    int ok = 0;
    if (argc == 4 && (strcmp(argv[1], "click") == 0 || strcmp(argv[1], "right-click") == 0)) {
        int x = coordinate(argv[2]), y = coordinate(argv[3]);
        unsigned int button = strcmp(argv[1], "click") == 0 ? 1 : 3;
        ok = x >= 0 && y >= 0 && click(display, x, y, button);
    } else if (argc == 4 && (strcmp(argv[1], "press") == 0 || strcmp(argv[1], "move") == 0)) {
        int x = coordinate(argv[2]), y = coordinate(argv[3]);
        ok = x >= 0 && y >= 0 && XTestFakeMotionEvent(display, DefaultScreen(display), x, y, CurrentTime);
        if (ok && strcmp(argv[1], "press") == 0)
            ok = XTestFakeButtonEvent(display, 1, True, CurrentTime);
    } else if (argc == 2 && strcmp(argv[1], "release") == 0) {
        ok = XTestFakeButtonEvent(display, 1, False, CurrentTime);
    } else if (argc == 3 && strcmp(argv[1], "key") == 0) {
        ok = tap(display, XStringToKeysym(argv[2]));
    } else if (argc == 4 && strcmp(argv[1], "chord") == 0) {
        KeySym modifier = XStringToKeysym(argv[2]);
        ok = press(display, modifier, True) && tap(display, XStringToKeysym(argv[3])) &&
             press(display, modifier, False);
    } else if (argc == 3 && strcmp(argv[1], "geometry") == 0) {
        ok = geometry(display, argv[2]);
    } else if (argc == 3 && strcmp(argv[1], "focus") == 0) {
        // What a window manager does when it activates a window: Xvfb has none.
        Window window = named(display, argv[2]);
        ok = window != None && XRaiseWindow(display, window) &&
             XSetInputFocus(display, window, RevertToParent, CurrentTime);
    } else if (argc == 2 && strcmp(argv[1], "copy") == 0) {
        ok = press(display, XK_Control_L, True) && tap(display, XK_a) &&
             tap(display, XK_c) && press(display, XK_Control_L, False);
    } else if (argc == 2 && strcmp(argv[1], "paste") == 0) {
        ok = paste(display);
    }
    XSync(display, False);
    XCloseDisplay(display);
    if (!ok)
        fprintf(stderr, "X11 input probe failed: %s\n", argv[1]);
    return ok ? EXIT_SUCCESS : EXIT_FAILURE;
}
