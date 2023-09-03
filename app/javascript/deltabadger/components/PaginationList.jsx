import React from 'react'

export const PaginationList = ({page, setPage, numberOfPages, handleCancel}) => {
  const getPage = (offset) => {
    if (numberOfPages <= 7) return offset + 1;

    if (page <= 4) return offset + 1;
    if (page >= numberOfPages - 3) return numberOfPages - 7 + offset + 1;

    return page - 4 + offset + 1;
  }

  const getClass = (i) => {
    const pageNum = getPage(i);
    return pageNum === page ? "page-item disabled" : "page-item";
  }

  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      <nav aria-label="bots pagination list">
        <ul className="pagination pagination-lg" style={{marginBottom: 0, paddingBottom: 0}}>
          {[...Array(7)].map((_, i) => (
            <li className={getClass(i)}>
              <a className="page-link" onClick={() => {setPage(getPage(i)); handleCancel()}}>
                {getPage(i)}
              </a>
            </li>
          ))}
        </ul>
      </nav>
    </div>
  )
}
