import React, { useState } from 'react'

export const BitBayInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0" role="alert">
      <b className="alert-heading mb-2">How to get API keys from Bitbay:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://auth.bitbay.net/ref/Hhb7ZrAv2GrA" target="_blank" rel="noopener">Bitbay</a> account.</li>
        <li>In user menu (top right corner) go to <b>API Settings</b>.</li>
        <li>Press <b>Add New Key</b>.</li>
        <li>Set permissions for:
          <ul>
          	<li><b>Get orders and market configurations</b></li>
          	<li><b>Manage orders and change market configurations</b></li>
          </ul>
        </li>
        <li>Press <b>Create</b>.</li>
        <li>Copy and paste your new API keys into the form above.</li>
      </ol>
    </div>
  </div>
)
