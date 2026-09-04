package dingtalk

import (
	"errors"
	"testing"

	db "github.com/patchbay-ai/patchbay/server/pkg/db/generated"
)

func TestValidateBindingTokenChannel(t *testing.T) {
	if err := validateBindingTokenChannel(db.ChannelBindingToken{ChannelType: string(TypeDingTalk)}); err != nil {
		t.Fatalf("DingTalk token rejected: %v", err)
	}
	if err := validateBindingTokenChannel(db.ChannelBindingToken{ChannelType: "slack"}); !errors.Is(err, ErrBindingTokenInvalid) {
		t.Fatalf("cross-channel token error = %v, want ErrBindingTokenInvalid", err)
	}
}
