import React from 'react'

export const PaginationList = ({page, setPage, numberOfPages, handleCancel}) => {
  const first = () => {
    if (page === 1)
      return 1

    if (page === numberOfPages) {
      return numberOfPages > 2 ? page - 2 : page - 1
    }

    if (page > 1 && page < numberOfPages) {
      return page - 1
    }
  }

  const second = () => first() + 1

  const third = () => {
    if (page === 1)
      return 3

    if (page === numberOfPages) {
      return page
    }

    if (page > 1 && page < numberOfPages) {
      return page + 1
    }
  }

  const firstClass = () => page === 1 ? "page-item disabled" : "page-item"
  const secondClass = () => {
    if ((page > 1 && page < numberOfPages) || (numberOfPages === 2 && page === 2)){
      return "page-item disabled"
    }

    return "page-item"
  }
  const thirdClass = () => page === numberOfPages ? "page-item disabled" : "page-item"

  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      <nav aria-label="bots pagination list">
        <ul className="pagination pagination-lg" style={{marginBottom: 0, paddingBottom: 0}}>
          <li className={firstClass()}><a className="page-link" onClick={() => {setPage(first()); handleCancel()}} tabIndex="-1">{first()}</a></li>
          <li className={secondClass()}><a className="page-link" onClick={() => {setPage(second()); handleCancel()}}>{second()}</a></li>
          { numberOfPages > 2 &&
            <li className={thirdClass()}><a className="page-link" onClick={() => {setPage(third()); handleCancel()}}>{third()}</a></li>
          }
        </ul>
      </nav>
    </div>
  )
}
