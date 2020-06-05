import React from 'react'

export const BitCludeInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0 mb-3 col" role="alert">
      <b className="alert-heading mb-2">How to get API keys from BitClude:</b>
      <hr/>
      <ol>
        <li>Login to your <a href="https://auth.bitbay.net/ref/Hhb7ZrAv2GrA" target="_blank" rel="noopener">BitClude</a> account.</li>
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
        <li>WAZNE: api/public key to id uzytkownika, private key to wygenerowany klucz</li>
        <li>WAZNE: wystarczy uprawnienie do wykonywania transakcji (2 od gory)</li>
      </ol>
    </div>
  </div>
)
