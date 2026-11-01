# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Peripheral Hub contributors
"""Authorization from the bus daemon and logind, never from supplied UID fields."""

from dataclasses import dataclass
import re

from validation import BackendError, integer


@dataclass(frozen=True)
class GraphicalSession:
    uid: int
    path: str


class Credentials:
    def __init__(self, unix_user_lookup, session_lookup):
        self.unix_user_lookup = unix_user_lookup
        self.session_lookup = session_lookup

    def caller_uid(self, call_info):
        sender = call_info.get("sender") if type(call_info) is dict else None
        if not isinstance(sender, str) or not re.fullmatch(r":[0-9]+\.[0-9]+", sender):
            raise BackendError("AccessDenied", "A real unique DBus sender is required")
        try:
            uid = self.unix_user_lookup(sender)
        except Exception:
            raise BackendError("AccessDenied", "DBus connection credentials could not be verified") from None
        if not integer(uid, 1):
            raise BackendError("AccessDenied", "Remapping requires a non-root user connection")
        return uid

    def active_session(self, uid):
        try:
            sessions = self.session_lookup(uid)
        except Exception:
            raise BackendError("AccessDenied", "Local graphical session could not be verified") from None
        if type(sessions) is not list:
            raise BackendError("AccessDenied", "logind returned invalid session metadata")
        candidates = [session for session in sessions if (
            type(session) is dict
            and integer(session.get("uid"), 1)
            and session.get("uid") == uid
            and session.get("active") is True
            and session.get("remote") is False
            and session.get("type") in ("x11", "wayland")
            and session.get("class") == "user"
            and session.get("state") == "active"
            and session.get("seat") == "seat0"
            and isinstance(session.get("path"), str)
            and session["path"].startswith("/org/freedesktop/login1/session/")
        )]
        if len(candidates) != 1:
            raise BackendError("AccessDenied", "Exactly one active, local, seat0 graphical user session is required")
        return GraphicalSession(uid, candidates[0]["path"])

    def session_is_active(self, session):
        try:
            return self.active_session(session.uid) == session
        except BackendError:
            return False
