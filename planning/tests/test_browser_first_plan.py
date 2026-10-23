import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


def read_plan(name):
    return (ROOT / "planning" / name).read_text()


class BrowserFirstPlanTests(unittest.TestCase):
    def test_b4_is_recorded_debt_not_a_phase_gate(self):
        roadmap = read_plan("10-roadmap-and-backlog.md")
        phase_a = next(line for line in roadmap.splitlines() if "| **P1a First useful slice**" in line)
        ui_epic = next(line for line in roadmap.splitlines() if "| E1.10 |" in line)
        p1_close = next(line for line in roadmap.splitlines() if "| **P1 close**" in line)
        critical_path = next(line for line in roadmap.splitlines() if "**Critical path" in line)

        self.assertIn("B7", phase_a)
        self.assertIn("B4 miss remains recorded deferred debt", phase_a)
        self.assertNotIn("B4, B7", phase_a)
        self.assertIn("B4 result recorded as deferred debt", ui_epic)
        self.assertNotIn("B4", p1_close.split("|")[4])
        self.assertIn("B4 remains deferred debt", p1_close)
        self.assertIn("B4 result is recorded deferred debt", critical_path)
        self.assertNotIn("B4 daemon-to-frame latency)", roadmap)

    def test_deferred_desktop_features_are_not_p1e_hardening(self):
        roadmap = read_plan("10-roadmap-and-backlog.md")
        p1e = next(line for line in roadmap.splitlines() if "| **P1e Hardening**" in line)
        hardening = next(line for line in roadmap.splitlines() if "| E1.18 |" in line)
        desktop_port = next(line for line in roadmap.splitlines() if "| E1.12 |" in line)

        for feature in ("menu bar", "keychain", "notifications", "connect-first", "version skew"):
            self.assertNotIn(feature, p1e)
        self.assertNotIn("M13", hardening)
        self.assertIn("Later desktop port", desktop_port)
        self.assertIn("version-skew UI", desktop_port)

    def test_enrollment_is_distinct_from_authentication_and_tested(self):
        security = read_plan("06-security.md")
        roadmap = read_plan("18-browser-first-roadmap.md")

        for text in (security, roadmap):
            self.assertIn("single-use", text)
            self.assertIn("128 bits of entropy", text)
            self.assertIn("five minutes", text)
            self.assertIn("limited", text)
            self.assertIn("wrong, expired, replayed", text)
            self.assertIn("unknown key", text)
        self.assertIn("concurrent code redemption", roadmap)
        self.assertIn("throttling", roadmap)

    def test_host_origin_boundary_is_exact_and_hostile_cases_are_named(self):
        security = read_plan("06-security.md")
        roadmap = read_plan("18-browser-first-roadmap.md")

        self.assertIn("127.0.0.1:<port>", security)
        self.assertIn("Origin: http://127.0.0.1:<port>", security)
        self.assertIn("binds only `127.0.0.1`", roadmap)
        self.assertIn("IPv6 `::1` and wildcard/dual-stack binds are unsupported", roadmap)
        for case in (
            "duplicate",
            "malformed",
            "lookalike/suffix",
            "`Origin: null`",
            "mismatched ports",
            "concurrent code redemption",
            "neither a WebSocket upgrade nor a handshake challenge",
        ):
            self.assertIn(case, roadmap)


if __name__ == "__main__":
    unittest.main()
