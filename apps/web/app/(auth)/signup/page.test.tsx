import { describe, expect, it } from 'vitest';
import LoginPage from '../login/page';
import SignupPage from './page';
import SignInAlias from '../../sign-in/[[...sign-in]]/page';
import SignUpAlias from '../../sign-up/[[...sign-up]]/page';
describe('single custom authentication entry', () => {
 it('uses the same email sign-in/sign-up component for every public entry', () => {
  expect(SignupPage).toBe(LoginPage); expect(SignInAlias).toBe(LoginPage); expect(SignUpAlias).toBe(LoginPage);
 });
});
