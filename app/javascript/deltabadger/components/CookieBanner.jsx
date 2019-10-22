import React from 'react'
import CookieConsent from "react-cookie-consent";

export const CookieBanner = () => {
  return (
    <CookieConsent
      location="bottom"
      buttonText="Sure man!!"
      cookieName="CookieConsentDeltabadger"
      style={{ background: "#2B373B" }}
      buttonStyle={{ color: "#4e503b", fontSize: "13px" }}
      expires={150}
    >
      This website uses cookies to enhance the user experience.{" "}
      <span style={{ fontSize: "10px" }}>
        This bit of text is smaller :O
      </span>
    </CookieConsent>
  )
}
