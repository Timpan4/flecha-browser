// Native key and pointer dispatch on the workflow harness's private X server,
// scoped by browser PID. Usage: x11-input DISPLAY PID then one of
//   title: print the browser window's title.
//   windows: print "x y width height id" of each mapped top-level window on
//   the display (the main window, bubbles, menus), in screen pixels, bottom
//   to top.
//   focus: print the id of the top-level window that has keyboard focus.
//   click X Y [BUTTON]: click at X,Y in the window (button 3 for a menu).
//   drag X1 Y1 X2 Y2: drag with the left button from X1,Y1 to X2,Y2.
//   down KEY... or up KEY...: press or release keys; focus stays put.
//   type TEXT: type ASCII text into whatever has focus (a dialog, say).
//   KEY...: focus the window and press the keys as one chord.
#include <X11/XKBlib.h>
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <X11/extensions/XTest.h>
#include <X11/keysym.h>

#include <algorithm>
#include <cstdlib>
#include <iostream>
#include <string>
#include <vector>

Window FindWindow(Display *display, Window root, unsigned long pid) {
  Atom type;
  int format;
  unsigned long count, remaining;
  unsigned char *data = nullptr;
  XGetWindowProperty(display, root, XInternAtom(display, "_NET_WM_PID", False),
                     0, 1, False, XA_CARDINAL, &type, &format, &count,
                     &remaining, &data);
  const bool matches =
      data && count && *reinterpret_cast<unsigned long *>(data) == pid;
  if (data)
    XFree(data);
  XWindowAttributes attrs;
  if (matches && XGetWindowAttributes(display, root, &attrs) &&
      attrs.map_state == IsViewable)
    return root;
  Window root_return, parent, *children = nullptr;
  unsigned int length;
  if (!XQueryTree(display, root, &root_return, &parent, &children, &length))
    return None;
  Window found = None;
  for (unsigned int i = 0; i < length && !found; ++i)
    found = FindWindow(display, children[i], pid);
  if (children)
    XFree(children);
  return found;
}

// Bubbles and menus are override-redirect windows without _NET_WM_PID, so
// this lists every mapped top-level window: the harness's display holds only
// its own browser.
void ListWindows(Display *display) {
  const Window root = DefaultRootWindow(display);
  Window root_return, parent, *children = nullptr;
  unsigned int length;
  if (!XQueryTree(display, root, &root_return, &parent, &children, &length))
    return;
  for (unsigned int i = 0; i < length; ++i) {
    XWindowAttributes attrs;
    if (!XGetWindowAttributes(display, children[i], &attrs) ||
        attrs.map_state != IsViewable || attrs.width <= 1 || attrs.height <= 1)
      continue;
    std::cout << attrs.x << ' ' << attrs.y << ' ' << attrs.width << ' '
              << attrs.height << ' ' << children[i] << '\n';
  }
  if (children)
    XFree(children);
}

// The top-level window holding keyboard focus, as ListWindows names it.
void PrintFocus(Display *display) {
  const Window root = DefaultRootWindow(display);
  Window focus;
  int revert;
  XGetInputFocus(display, &focus, &revert);
  while (focus != None && focus != PointerRoot && focus != root) {
    Window root_return, parent, *children = nullptr;
    unsigned int length;
    if (!XQueryTree(display, focus, &root_return, &parent, &children, &length))
      return;
    if (children)
      XFree(children);
    if (parent == root) {
      std::cout << focus << '\n';
      return;
    }
    focus = parent;
  }
}

std::vector<KeyCode> KeyCodes(Display *display, int argc, char **argv,
                              int first) {
  std::vector<KeyCode> keys;
  for (int i = first; i < argc; ++i) {
    const KeyCode code = XKeysymToKeycode(display, XStringToKeysym(argv[i]));
    if (!code)
      return {};
    keys.push_back(code);
  }
  return keys;
}

int main(int argc, char **argv) {
  if (argc < 4)
    return 1;
  Display *display = XOpenDisplay(argv[1]);
  if (!display)
    return 1;
  const Window window =
      FindWindow(display, DefaultRootWindow(display), std::stoul(argv[2]));
  if (!window)
    return 1;
  const std::string mode = argv[3];
  if (mode == "windows") {
    ListWindows(display);
  } else if (mode == "focus") {
    PrintFocus(display);
  } else if (mode == "click" && argc >= 6) {
    int x, y;
    Window child;
    XTranslateCoordinates(display, window, DefaultRootWindow(display),
                          std::stoi(argv[4]), std::stoi(argv[5]), &x, &y,
                          &child);
    const unsigned int button = argc > 6 ? std::stoul(argv[6]) : 1;
    XTestFakeMotionEvent(display, -1, x, y, CurrentTime);
    XTestFakeButtonEvent(display, button, True, CurrentTime);
    XTestFakeButtonEvent(display, button, False, CurrentTime);
    XSync(display, False);
  } else if (mode == "drag" && argc >= 8) {
    // Left button down at the first point, through every pixel on the way, up
    // at the second: the motion a real drag sends.
    int x1, y1, x2, y2;
    Window child;
    XTranslateCoordinates(display, window, DefaultRootWindow(display),
                          std::stoi(argv[4]), std::stoi(argv[5]), &x1, &y1,
                          &child);
    XTranslateCoordinates(display, window, DefaultRootWindow(display),
                          std::stoi(argv[6]), std::stoi(argv[7]), &x2, &y2,
                          &child);
    XTestFakeMotionEvent(display, -1, x1, y1, CurrentTime);
    XTestFakeButtonEvent(display, 1, True, CurrentTime);
    const int steps = std::max(std::abs(x2 - x1), std::abs(y2 - y1));
    for (int i = 1; i <= steps; ++i) {
      XTestFakeMotionEvent(display, -1, x1 + (x2 - x1) * i / steps,
                           y1 + (y2 - y1) * i / steps, CurrentTime);
      XSync(display, False);
    }
    XTestFakeButtonEvent(display, 1, False, CurrentTime);
    XSync(display, False);
  } else if (mode == "type" && argc >= 5) {
    const KeyCode shift = XKeysymToKeycode(display, XK_Shift_L);
    for (const char c : std::string(argv[4])) {
      const char name[2] = {c, 0};
      KeySym sym = XStringToKeysym(name);
      if (sym == NoSymbol) {
        // Punctuation has no one-letter name; Latin-1 keysyms are its code.
        sym = static_cast<unsigned char>(c);
      }
      const KeyCode code = XKeysymToKeycode(display, sym);
      if (!code)
        return 1;
      const bool shifted = XkbKeycodeToKeysym(display, code, 0, 0) != sym;
      if (shifted)
        XTestFakeKeyEvent(display, shift, True, CurrentTime);
      XTestFakeKeyEvent(display, code, True, CurrentTime);
      XTestFakeKeyEvent(display, code, False, CurrentTime);
      if (shifted)
        XTestFakeKeyEvent(display, shift, False, CurrentTime);
    }
    XSync(display, False);
  } else if (mode == "down" || mode == "up") {
    const std::vector<KeyCode> keys = KeyCodes(display, argc, argv, 4);
    if (keys.empty())
      return 1;
    for (const KeyCode code : keys)
      XTestFakeKeyEvent(display, code, mode == "down", CurrentTime);
    XSync(display, False);
  } else if (mode == "title") {
    const Atom property = XInternAtom(display, "_NET_WM_NAME", False);
    const Atom utf8 = XInternAtom(display, "UTF8_STRING", False);
    Atom type;
    int format;
    unsigned long count, remaining;
    unsigned char *data = nullptr;
    XGetWindowProperty(display, window, property, 0, 0, False, utf8, &type,
                       &format, &count, &remaining, &data);
    if (data)
      XFree(data);
    data = nullptr;
    // XGetWindowProperty measures its requested length in 32-bit units.
    XGetWindowProperty(display, window, property, 0, (remaining + 3) / 4, False,
                       utf8, &type, &format, &count, &remaining, &data);
    if (data && format == 8)
      std::cout.write(reinterpret_cast<char *>(data), count);
    if (data)
      XFree(data);
  } else {
    XSetInputFocus(display, window, RevertToParent, CurrentTime);
    const std::vector<KeyCode> keys = KeyCodes(display, argc, argv, 3);
    if (keys.empty())
      return 1;
    for (const KeyCode code : keys)
      XTestFakeKeyEvent(display, code, True, CurrentTime);
    for (auto it = keys.rbegin(); it != keys.rend(); ++it)
      XTestFakeKeyEvent(display, *it, False, CurrentTime);
    XSync(display, False);
  }
  XCloseDisplay(display);
}
