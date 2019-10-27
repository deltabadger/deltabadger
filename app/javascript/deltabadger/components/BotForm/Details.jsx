import React from 'react'
import { Info } from '../BotDetails/Info';

export const Details = ({ bot }) => {
  return (
    <div className="db-bots__item db-bot-data">
      <ul className="nav nav-tabs" id="myTab" role="tablist">
        <li className="nav-item">
          <a className="nav-link active" id="info-tab" data-toggle="tab" href="#info"  role="tab" aria-controls="info"  aria-selected="false">Info</a>
        </li>
      </ul>
      <div className="tab-content" id="myTabContent">
        <Info bot={bot} />
      </div>
    </div>
  )
}
