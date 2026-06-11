// Host-go tests for the mesh client half of the runtime (mesh.go). Split
// from runtime_test.go because mesh.go is only emitted when the module's
// WIT world has imports — runtime_test.go must compile without it. Shares
// test helpers (mustHex) with runtime_test.go: both live in package gen and
// always ship together when mesh.go is present.
package gen

import (
	"fmt"
	"strings"
	"testing"
)

func TestParseMeshReply(t *testing.T) {
	// status 0: body returned verbatim.
	body, err := parseMeshReply(mustHex(t, "0007"))
	if err != nil || fmt.Sprintf("%x", body) != "07" {
		t.Errorf("status-0 reply = %x, %v; want 07", body, err)
	}
	// status 0 with empty body: ok, empty result.
	body, err = parseMeshReply(mustHex(t, "00"))
	if err != nil || len(body) != 0 {
		t.Errorf("status-0 empty reply = %x, %v; want empty", body, err)
	}
	// status 1: the decoded error string.
	payload := append([]byte{1}, EncodeString(nil, "boom")...)
	if _, err = parseMeshReply(payload); err == nil || err.Error() != "boom" {
		t.Errorf("status-1 reply err = %v; want boom", err)
	}
	// status 1 with trailing bytes after the error string: malformed.
	if _, err = parseMeshReply(append(payload, 0)); err == nil ||
		!strings.Contains(err.Error(), "malformed error reply") {
		t.Errorf("status-1 trailing bytes err = %v; want malformed", err)
	}
	// status 1 with an undecodable string: malformed.
	if _, err = parseMeshReply(mustHex(t, "0105")); err == nil ||
		!strings.Contains(err.Error(), "malformed error reply") {
		t.Errorf("status-1 truncated string err = %v; want malformed", err)
	}
	// invalid status byte.
	if _, err = parseMeshReply(mustHex(t, "02")); err == nil ||
		!strings.Contains(err.Error(), "invalid reply status") {
		t.Errorf("status-2 err = %v; want invalid reply status", err)
	}
	// empty payload.
	if _, err = parseMeshReply(nil); err == nil ||
		!strings.Contains(err.Error(), "empty reply") {
		t.Errorf("empty payload err = %v; want empty reply", err)
	}
}
