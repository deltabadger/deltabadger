import React from 'react'
import CookieConsent from "react-cookie-consent";

export const CookieBanner = () => {
  return (
    <CookieConsent
      location="bottom"
      buttonText="I understand."
      cookieName="CookieConsentDeltabadger"
      style={{}}
      buttonStyle={{}}
      expires={150}
    >
      <span className="cookie_text">This website uses cookies to enhance the user experience.{" "}</span>
    </CookieConsent>
  )
}
