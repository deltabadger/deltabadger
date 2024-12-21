import React, { useEffect, useRef } from 'react';
import ReactDOM from 'react-dom';

export const Modal = ({ children, onClose }) => {
  const modalRef = useRef();

  useEffect(() => {
    const handleClickOutside = (e) => {
      if (modalRef.current && !modalRef.current.contains(e.target)) {
        onClose();
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [onClose]);

  if (!children) return null;

  return ReactDOM.createPortal(
    <div className="react-modal">
      <div ref={modalRef} className="react-modal__content">
        {children}
      </div>
    </div>,
    document.body
  );
}; 