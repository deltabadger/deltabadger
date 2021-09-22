import React from 'react'
import I18n from 'i18n-js'

export const ClosedForm = ({ handleSubmit }) => (
  <div className="db-bots__item d-flex justify-content-center db-add-more-bots">
    <button onClick={handleSubmit} className="btn btn-primary">
      {I18n.t('bots.add_new_bot')} +
    </button>
    <div className="db-bots__item d-flex db-add-more-bots">
      <nav aria-label="...">
        <ul className="pagination pagination-lg" style={{marginBottom: 0, paddingBottom: 0}}>
          <li className="page-item disabled">
            <a className="page-link" href="#" tabIndex="-1">1</a>
          </li>
          <li className="page-item"><a className="page-link" href="#">2</a></li>
          <li className="page-item"><a className="page-link" href="#">3</a></li>
        </ul>
      </nav>
    </div>
  </div>
)
