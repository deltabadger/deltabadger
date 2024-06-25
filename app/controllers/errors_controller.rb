class ErrorsController < ApplicationController
  def redirect_to_root
    redirect_to root_path
  end

  def unprocessable_entity
    render file: "#{Rails.root}/public/422.html", status: :unprocessable_entity, layout: false
  end

  def internal_server_error
    render file: "#{Rails.root}/public/500.html", status: :internal_server_error, layout: false
  end
end