import React from 'react'

export const PaginationList = ({page, setPage}) => (
  <div className="db-bots__item d-flex db-add-more-bots">
    <nav aria-label="bots pagination list">
        { page === 1 &&
          <ul className="pagination pagination-lg" style={{marginBottom: 0, paddingBottom: 0}}>
            <li className="page-item disabled"><a className="page-link" tabIndex="-1">{page}</a></li>
            <li className="page-item"><a className="page-link" onClick={() => setPage(page + 1)}>{page + 1}</a></li>
            <li className="page-item"><a className="page-link" onClick={() => setPage(page + 1)}>{page + 2}</a></li>
          </ul>
        }
        { page > 1 &&
          <ul className="pagination pagination-lg" style={{marginBottom: 0, paddingBottom: 0}}>
            <li className="page-item"><a className="page-link" onClick={() => setPage(page - 1)}>{page - 1}</a></li>
            <li className="page-item disabled"><a className="page-link" tabIndex="-1">{page}</a></li>
            <li className="page-item"><a className="page-link" onClick={() => setPage(page + 1)}>{page + 1}</a></li>
          </ul>
        }
    </nav>
  </div>
)
