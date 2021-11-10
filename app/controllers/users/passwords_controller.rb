class Users::PasswordsController < Devise::PasswordsController
  prepend_before_action :check_captcha, only: [:create]

  def check_captcha
    return if verify_recaptcha

    self.resource = resource_class.new
    respond_with_navigational(resource) { render :new }
  end
end
