package middleware

import (
	"crypto/subtle"
	"errors"
	"log/slog"
	"net"
	"net/http"
	"net/netip"
	"os"
	"strings"
)

const (
	identityProxyMarkerHeader   = "X-Cordy-Identity-Proxy-Token"
	identityProxyMinSecretBytes = 32
)

type identityProxyTrust struct {
	trustedPeers []netip.Prefix
	marker       []byte
}

type proxyIdentity struct {
	userID string
	email  string
}

func identityProxyTrustFromEnv() identityProxyTrust {
	trust, err := configuredIdentityProxyTrust(
		os.Getenv("CORDY_IDENTITY_TRUSTED_PROXIES"),
		os.Getenv("CORDY_IDENTITY_PROXY_SECRET"),
	)
	if err != nil {
		slog.Warn("auth: identity proxy trust disabled", "error", err)
		return identityProxyTrust{}
	}
	return trust
}

func configuredIdentityProxyTrust(cidrs, marker string) (identityProxyTrust, error) {
	if strings.TrimSpace(cidrs) == "" && marker == "" {
		return identityProxyTrust{}, nil
	}
	if strings.TrimSpace(cidrs) == "" || len(marker) < identityProxyMinSecretBytes {
		return identityProxyTrust{}, errors.New("CORDY_IDENTITY_TRUSTED_PROXIES and a 32-byte CORDY_IDENTITY_PROXY_SECRET are both required")
	}

	var trustedPeers []netip.Prefix
	for _, value := range strings.Split(cidrs, ",") {
		value = strings.TrimSpace(value)
		if value == "" {
			continue
		}
		prefix, err := netip.ParsePrefix(value)
		if err != nil {
			return identityProxyTrust{}, errors.New("CORDY_IDENTITY_TRUSTED_PROXIES contains an invalid CIDR")
		}
		trustedPeers = append(trustedPeers, prefix)
	}
	if len(trustedPeers) == 0 {
		return identityProxyTrust{}, errors.New("CORDY_IDENTITY_TRUSTED_PROXIES must contain at least one CIDR")
	}

	return identityProxyTrust{
		trustedPeers: trustedPeers,
		marker:       []byte(marker),
	}, nil
}

func (trust identityProxyTrust) takeIdentity(r *http.Request) *proxyIdentity {
	userID := strings.TrimSpace(r.Header.Get("X-User-ID"))
	email := strings.TrimSpace(r.Header.Get("X-User-Email"))
	marker := r.Header.Get(identityProxyMarkerHeader)
	trusted := trust.containsDirectPeer(r.RemoteAddr) &&
		len(marker) == len(trust.marker) &&
		subtle.ConstantTimeCompare([]byte(marker), trust.marker) == 1 &&
		userID != ""

	// Authentication owns these headers. Always discard request copies,
	// then restore only identity established by the trusted proxy boundary.
	r.Header.Del("X-User-ID")
	r.Header.Del("X-User-Email")
	r.Header.Del(identityProxyMarkerHeader)
	if !trusted {
		return nil
	}

	r.Header.Set("X-User-ID", userID)
	if email != "" {
		r.Header.Set("X-User-Email", email)
	}
	return &proxyIdentity{userID: userID, email: email}
}

func (trust identityProxyTrust) containsDirectPeer(remoteAddr string) bool {
	host, _, err := net.SplitHostPort(remoteAddr)
	if err != nil {
		host = remoteAddr
	}
	ip, err := netip.ParseAddr(host)
	if err != nil {
		return false
	}
	for _, prefix := range trust.trustedPeers {
		if prefix.Contains(ip) {
			return true
		}
	}
	return false
}
