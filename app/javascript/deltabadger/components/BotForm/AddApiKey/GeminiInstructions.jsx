import React, { useState } from 'react'

export const GeminiInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys from Gemini:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://exchange.gemini.com/signin" target="_blank" rel="nofollow">Gemini</a> account.</li>
        <li>In account menu (top right corner) go to <b>Settings</b>.</li>
        <li>Pick <b>API</b> (left side).</li>
        <li>Press <b>Create a new API key</b>.</li>
        <li>From the dropdown <b>Scope</b> pick <b>Primary</b>.</li>
        <li>Press <b>Create a new API key</b>.</li>
        <li>Set API key settings:
          <ul>
            <li><b>Trading</b></li>
          </ul>
        </li>
        <li>Do not check <b>Require session heartbeat</b></li>
        <li>Copy and paste your new API keys into the form above.</li>
      </ol>
    </div>
  </div>
)
