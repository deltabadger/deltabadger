import React, { useState } from 'react'

export const Spinner = () => (
  <div className="db-spinner-positioner">
    <svg className="spinner" width="2rem" height="2rem" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
      <circle className="path" fill="none" strokeWidth="4" strokeLinecap="round" cx="12" cy="12" r="10"></circle>
    </svg>
  </div>
)
