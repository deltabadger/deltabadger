import React from 'react'
export const CancelButton = ({handleCancel}) => {
  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      <button onClick={() => handleCancel()} className="btn btn-outline-primary">
        <i className="material-icons-round">close</i>
      </button>
    </div>
  )
}
