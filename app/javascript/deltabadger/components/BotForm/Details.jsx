import React from 'react'

export const Details = ({ bot }) => {
  return (
    <div className="db-bots__item db-bot-data">
      <ul className="nav nav-tabs" id="botFormInfo" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="botFormInfoTab" data-toggle="tab" href="#botFormInfoTab"  role="tab" aria-controls="botFormInfoTab"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="botFormInfo">
        <div className="tab-pane show active pl-3 pr-3" id="botFormInfoTab" role="tabpanel" aria-labelledby="botFormInfoTab">
          <p>Info</p>
        </div>
      </div>
    </div>
  )
}
