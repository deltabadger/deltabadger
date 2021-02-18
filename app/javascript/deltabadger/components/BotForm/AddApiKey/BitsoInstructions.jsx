import React from 'react'

export const BitsoInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys from Gemini:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://bitso.com/" target="_blank" rel="nofollow">Bitso</a> account.</li>
        <li>In account menu (top right corner) go to <b>Profile</b>.</li>
        <li>Pick <b>API</b> (left side).</li>
        <li>Press <b>Add new API</b>.</li>
        <li>Fill <b>API Name</b> and provide your <b>Transaction PIN</b>.</li>
        <li>Set Permissions:
          <ul>
            <li><b>Perform trades</b></li>
            <li><b>View balances</b></li>
            <li><b>View account information</b></li>
          </ul>
        </li>
        <li>Press <b>Save</b></li>
        <li>Copy and paste your new API keys into the form above.</li>
      </ol>
    </div>
  </div>
)
