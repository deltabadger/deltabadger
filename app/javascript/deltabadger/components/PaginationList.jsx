import React from 'react';

export const PaginationList = ({ page, setPage, numberOfPages, handleCancel }) => {

  const calculatePages = () => {
    if (numberOfPages <= 7) {
      return Array.from({ length: numberOfPages }, (_, i) => i + 1);
    } else {
      if (page < 4) {
        return Array.from({ length: 7 }, (_, i) => i + 1);
      }
      if (page >= numberOfPages - 3) {
        return Array.from({ length: 7 }, (_, i) => numberOfPages - 7 + i);
      }
      let startPage = page - 3;
      return Array.from({ length: 7 }, (_, i) => startPage + i);
    }
  };

  const handlePageClick = (pageNumber) => {
    setPage(pageNumber);
    handleCancel();
  };

  const pages = calculatePages();
  const pageClasses = (pageNumber) =>
    page === pageNumber ? 'page-item disabled' : 'page-item';

  return (
    <div className="db-bots__item d-flex db-add-more-bots">
      <nav aria-label="bots pagination list">
        <ul className="pagination pagination-lg" style={{ marginBottom: 0, paddingBottom: 0 }}>
          {pages.map((pageNumber) => (
            <li className={pageClasses(pageNumber)} key={pageNumber}>
              <a
                className="page-link"
                onClick={() => handlePageClick(pageNumber)}
                tabIndex="-1"
              >
                {pageNumber}
              </a>
            </li>
          ))}
        </ul>
      </nav>
    </div>
  );
};
