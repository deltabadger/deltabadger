import React from 'react'

export const Progressbar = ({ value }) => (
  <div className="db-bot__progress progress progress--thin progress--bot-setup">
    <div className="progress-bar" role="progressbar" style={{width: `${value}%`}} aria-valuenow={value} aria-valuemin="0" aria-valuemax="100" />
 </div>
)
