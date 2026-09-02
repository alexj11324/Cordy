package middleware

import (
	"go/ast"
	"go/parser"
	"go/token"
	"path/filepath"
	"runtime"
	"testing"
)

func guestRouteCallee(expr ast.Expr) (string, string) {
	selector, ok := expr.(*ast.SelectorExpr)
	if !ok {
		return "", ""
	}
	if ident, ok := selector.X.(*ast.Ident); ok {
		return ident.Name, selector.Sel.Name
	}
	return "", selector.Sel.Name
}

func guestRouteString(expr ast.Expr) string {
	lit, ok := expr.(*ast.BasicLit)
	if !ok || lit.Kind != token.STRING || len(lit.Value) < 2 {
		return ""
	}
	return lit.Value[1 : len(lit.Value)-1]
}

type guestRouteRange struct {
	start token.Pos
	end   token.Pos
}

// TestGuestRouteContract locks the security placement of the guest lifecycle
// routes. They must share the normal Auth group, carry the human-actor gate,
// and remain before the workspace membership group because guest accounts may
// not have a workspace yet. The public creation/logout contracts are checked
// at the same source boundary so a future route refactor cannot silently drop
// one half of the bearer lifecycle.
func TestGuestRouteContract(t *testing.T) {
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("runtime.Caller failed")
	}
	routerFile := filepath.Join(filepath.Dir(thisFile), "..", "..", "cmd", "server", "router.go")
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, routerFile, nil, 0)
	if err != nil {
		t.Fatalf("parse %s: %v", routerFile, err)
	}

	var routerFunc *ast.FuncDecl
	for _, decl := range file.Decls {
		fn, ok := decl.(*ast.FuncDecl)
		if ok && fn.Name.Name == "NewRouterWithOptions" {
			routerFunc = fn
			break
		}
	}
	if routerFunc == nil || routerFunc.Body == nil {
		t.Fatalf("NewRouterWithOptions not found in %s", routerFile)
	}

	var (
		guestAuthRoute      *ast.CallExpr
		logoutRoute         *ast.CallExpr
		guestSessionsRoute  *ast.CallExpr
		guestRouteHasHuman  bool
		guestRouteHasCreate bool
		guestRouteHasGet    bool
		guestRouteHasClaim  bool
		guestRouteHasRevoke bool
		authGroupRanges     []guestRouteRange
		workspaceRanges     []guestRouteRange
	)
	ast.Inspect(routerFunc.Body, func(node ast.Node) bool {
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		owner, method := guestRouteCallee(call.Fun)
		switch {
		case method == "Post" && len(call.Args) >= 2 && guestRouteString(call.Args[0]) == "/auth/guest":
			guestAuthRoute = call
		case method == "Post" && len(call.Args) >= 2 && guestRouteString(call.Args[0]) == "/auth/logout":
			logoutRoute = call
		case method == "Route" && len(call.Args) >= 2 && guestRouteString(call.Args[0]) == "/api/guest-sessions":
			guestSessionsRoute = call
		case owner == "r" && method == "Group" && len(call.Args) == 1:
			callback, ok := call.Args[0].(*ast.FuncLit)
			if !ok || callback.Body == nil {
				break
			}
			groupRange := guestRouteRange{start: callback.Body.Pos(), end: callback.Body.End()}
			if guestRouteFunctionUses(callback, "middleware", "Auth") {
				authGroupRanges = append(authGroupRanges, groupRange)
			}
			if guestRouteFunctionUses(callback, "middleware", "RequireWorkspaceMember") {
				workspaceRanges = append(workspaceRanges, groupRange)
			}
		}
		return true
	})

	if guestAuthRoute == nil {
		t.Error("/auth/guest is not registered")
	} else if !guestPostUsesRateLimit(guestAuthRoute, "authRL") {
		t.Error("/auth/guest is not wrapped with authRL")
	}

	if logoutRoute == nil {
		t.Error("/auth/logout is not registered")
	} else if !guestPostUsesMiddleware(logoutRoute, "middleware", "RevokeGuestOnLogout") {
		t.Error("/auth/logout is not wrapped with middleware.RevokeGuestOnLogout")
	}

	if guestSessionsRoute == nil {
		t.Fatal("/api/guest-sessions is not registered")
	}
	if !guestRouteInRanges(guestSessionsRoute.Pos(), authGroupRanges) {
		t.Errorf("/api/guest-sessions is not inside the normal middleware.Auth group at %s", fset.Position(guestSessionsRoute.Pos()))
	}
	if guestRouteInRanges(guestSessionsRoute.Pos(), workspaceRanges) {
		t.Errorf("/api/guest-sessions is nested inside a workspace membership group at %s", fset.Position(guestSessionsRoute.Pos()))
	}
	if callback, ok := guestSessionsRoute.Args[1].(*ast.FuncLit); ok {
		ast.Inspect(callback.Body, func(node ast.Node) bool {
			call, ok := node.(*ast.CallExpr)
			if !ok {
				return true
			}
			owner, method := guestRouteCallee(call.Fun)
			if owner == "r" && method == "Use" && len(call.Args) == 1 && guestRouteUsesMiddleware(call.Args[0], "handler", "RequireHumanActor") {
				guestRouteHasHuman = true
			}
			if method == "Post" && len(call.Args) >= 2 {
				switch guestRouteString(call.Args[0]) {
				case "/":
					guestRouteHasCreate = true
				case "/claim":
					guestRouteHasClaim = true
				case "/revoke":
					guestRouteHasRevoke = true
				}
			}
			if method == "Get" && len(call.Args) >= 2 && guestRouteString(call.Args[0]) == "/" {
				guestRouteHasGet = true
			}
			return true
		})
	}
	if !guestRouteHasHuman {
		t.Error("guest lifecycle route is missing handler.RequireHumanActor")
	}
	if !guestRouteHasCreate || !guestRouteHasGet || !guestRouteHasClaim || !guestRouteHasRevoke {
		t.Errorf("guest lifecycle routes incomplete: create=%v get=%v claim=%v revoke=%v", guestRouteHasCreate, guestRouteHasGet, guestRouteHasClaim, guestRouteHasRevoke)
	}
}

func guestRouteFunctionUses(fn *ast.FuncLit, packageName, functionName string) bool {
	var found bool
	ast.Inspect(fn.Body, func(node ast.Node) bool {
		if found {
			return false
		}
		call, ok := node.(*ast.CallExpr)
		if !ok {
			return true
		}
		owner, method := guestRouteCallee(call.Fun)
		if owner == "r" && (method == "Group" || method == "Route") {
			// Nested route callbacks belong to their own middleware scope. Do
			// not mistake a workspace group's Use call for the enclosing Auth
			// group's direct middleware.
			return false
		}
		if owner == "r" && method == "Use" && len(call.Args) == 1 && guestRouteUsesMiddleware(call.Args[0], packageName, functionName) {
			found = true
		}
		return true
	})
	return found
}

func guestRouteInRanges(pos token.Pos, ranges []guestRouteRange) bool {
	for _, routeRange := range ranges {
		if pos >= routeRange.start && pos <= routeRange.end {
			return true
		}
	}
	return false
}

func guestPostUsesRateLimit(call *ast.CallExpr, variable string) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || selector.Sel.Name != "Post" {
		return false
	}
	withCall, ok := selector.X.(*ast.CallExpr)
	if !ok {
		return false
	}
	withSelector, ok := withCall.Fun.(*ast.SelectorExpr)
	if !ok || withSelector.Sel.Name != "With" || len(withCall.Args) != 1 {
		return false
	}
	ident, ok := withCall.Args[0].(*ast.Ident)
	return ok && ident.Name == variable
}

func guestPostUsesMiddleware(call *ast.CallExpr, packageName, functionName string) bool {
	selector, ok := call.Fun.(*ast.SelectorExpr)
	if !ok || selector.Sel.Name != "Post" {
		return false
	}
	withCall, ok := selector.X.(*ast.CallExpr)
	if !ok || len(withCall.Args) != 1 {
		return false
	}
	return guestRouteUsesMiddleware(withCall.Args[0], packageName, functionName)
}

func guestRouteUsesMiddleware(expr ast.Expr, packageName, functionName string) bool {
	if owner, name := guestRouteCallee(expr); owner != "" || name != "" {
		return owner == packageName && name == functionName
	}
	call, ok := expr.(*ast.CallExpr)
	if !ok {
		return false
	}
	owner, name := guestRouteCallee(call.Fun)
	return owner == packageName && name == functionName
}
