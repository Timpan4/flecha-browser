#include <X11/Xlib.h>
#include <X11/extensions/XTest.h>
#include <X11/keysym.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

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

static int click(Display *display, int x, int y)
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
    return XTestFakeButtonEvent(display, 1, True, CurrentTime) &&
           XTestFakeButtonEvent(display, 1, False, CurrentTime);
}

static int paste(Display *display)
{
    Atom clipboard = XInternAtom(display, "CLIPBOARD", False);
    if (XGetSelectionOwner(display, clipboard) == None)
        return 0;
    Atom utf8 = XInternAtom(display, "UTF8_STRING", False);
    Atom property = XInternAtom(display, "FLECHA_SPIKE", False);
    Window requestor = XCreateSimpleWindow(display, DefaultRootWindow(display),
                                          0, 0, 1, 1, 0, 0, 0);
    XConvertSelection(display, clipboard, utf8, property, requestor, CurrentTime);
    XEvent event;
    do {
        XNextEvent(display, &event);
    } while (event.type != SelectionNotify);
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
    if (argc == 4 && strcmp(argv[1], "click") == 0) {
        int x = coordinate(argv[2]), y = coordinate(argv[3]);
        ok = x >= 0 && y >= 0 && click(display, x, y);
    } else if (argc == 3 && strcmp(argv[1], "key") == 0) {
        ok = tap(display, XStringToKeysym(argv[2]));
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
