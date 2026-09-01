# Two-factor authentication

Two-factor authentication adds a six-digit code from an authenticator app to your login. It protects the account that holds your exchange keys, so turn it on if anyone else can reach the address your Deltabadger runs on.

## Enabling

1. Open **Settings → Account**. The **Second Factor Authentication** widget shows ON or OFF.
2. Press **Enable 2FA**. A dialog titled **2FA** opens with a QR code. Scan it with any TOTP authenticator app (Google Authenticator, Aegis, 1Password, …), or copy the code printed under it and add it by hand. The entry appears in the app as Deltabadger.
3. Type the six digits the app shows into **Enter 2FA code** and press **Enable 2FA**.

A wrong code is refused with "Supplied code is wrong." and nothing changes.

There are no recovery codes. If you lose the authenticator you cannot sign in; keep the app backed up or the secret written down somewhere safe.

## Logging in

After a correct email and password you land on a page asking for the **2FA code**. Enter it and press **Verify**. You have five minutes; after that the pending login is dropped and you are sent back to the login page with "Invalid code. Try again." — sign in once more. **Remember me** ticked on the login page still applies once the code is accepted.

Each code works once. A device whose clock is off by up to about half a minute is still accepted.

## Disabling

Press **Disable 2FA** on the Account page, enter the current code from your app and press **Disable 2FA** in the dialog. The switch turns OFF and the next login needs only the password.

## Password reset

When you reset a forgotten password from the emailed link, the **Change password** form has an extra **2FA code** field. The reset only goes through with a valid code. Without the authenticator, the password cannot be reset this way.

## Lockout

Five wrong codes — at login or on the password-reset form — lock the account for 15 minutes. At login the fifth one sends you back to the login page with "Invalid code. Try again."; only the next sign-in attempt says the account is locked. Wrong passwords count toward the same five. A locked account is not shown the code page at all; wait out the 15 minutes and try again. There is no unlock-by-email.

## Losing the secret

The two-factor secret is encrypted with the instance key. The credential reset described in [Secrets and encryption keys](40-secrets-and-encryption-keys.md) clears it along with everything else, which is the only built-in way to remove two-factor from an account you can no longer verify.
