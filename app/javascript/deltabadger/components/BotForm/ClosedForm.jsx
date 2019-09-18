import React, { useState } from 'react'

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
    <button onClick={handleSubmit} className="btn btn-link">
      Add new bot +
    </button>
  </div>
)
