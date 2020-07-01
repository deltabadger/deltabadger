import React from 'react'

export const BitCludeInstructions = () => (
  <div className="db-exchange-instructions db-exchange-instructions--bitbay">
    <div className="alert alert-success mx-0 mb-3 col" role="alert">
      <b className="alert-heading mb-2">Jak pozyskać klucze API?</b>
      <hr/>
      <ol>
        <li>Zaloguj się do swojego konta na <a href="https://bitclude.com/r/345469148" target="_blank" rel="noopener">BitClude</a>.</li>
        <li>W menu użytkownika w prawym górnym rogu wejdź w opcję <b>Klucze API</b>.</li>
        <li>Skopiuj <b>ID użytkownika</b> i wkej w pierwsze pole powyżej.</li>
        <li>W sekcji "Utwórz klucz API" włącz opcję <b>Wykonywanie transakcji</b>.</li>
        <li>Wciśnij przycisk <b>Utwórz Klucz</b>.</li>
        <li>Podaj hasło i kod SMS.</li>
        <li>Skopiuj wygenerowany klucz i wklej w drugie pole powyżej.</li>
      </ol>
    </div>
  </div>
)
