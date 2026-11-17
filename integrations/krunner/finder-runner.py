#!/usr/bin/env python3
"""KRunner D-Bus runner: always offers 'Search "<query>" on Finder'."""
import argparse
import subprocess
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

BUS_NAME = "org.finder.krunner"
OBJ_PATH = "/runner"
IFACE = "org.kde.krunner1"
POSSIBLE_MATCH = 30


class Runner(dbus.service.Object):
    def __init__(self, bus, finder_bin):
        super().__init__(bus, OBJ_PATH)
        self.finder_bin = finder_bin

    @dbus.service.method(IFACE, in_signature="s", out_signature="a(sssida{sv})")
    def Match(self, query):
        query = query.strip()
        if not query:
            return []
        return [(query, f'Search "{query}" on Finder', "system-search",
                 dbus.Int32(POSSIBLE_MATCH), 0.2, {})]

    @dbus.service.method(IFACE, in_signature="", out_signature="a(sss)")
    def Actions(self):
        return []

    @dbus.service.method(IFACE, in_signature="ss", out_signature="")
    def Run(self, match_id, action_id):
        subprocess.Popen([self.finder_bin, match_id], start_new_session=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--finder", required=True, help="path to the finder binary")
    args = ap.parse_args()
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    name = dbus.service.BusName(BUS_NAME, bus)  # noqa: F841 (keep reference)
    Runner(bus, args.finder)
    GLib.MainLoop().run()


if __name__ == "__main__":
    sys.exit(main())
