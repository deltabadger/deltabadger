require 'rails_helper'

RSpec.describe "Authentication", type: :request do
  let!(:admin) { create(:user, admin: true) }

  describe "login" do
    let(:user) { create(:user, password: "SecurePass1!") }

    it "signs in with valid credentials" do
      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }

      expect(response).to be_redirect
      follow_redirect!
      expect(controller.current_user).to eq(user)
    end

    it "rejects invalid password" do
      post user_session_path, params: {
        user: { email: user.email, password: "wrongpassword" }
      }

      expect(response).to have_http_status(:unprocessable_content)
    end

    it "rejects unknown email" do
      post user_session_path, params: {
        user: { email: "unknown@example.com", password: "SecurePass1!" }
      }

      expect(response).to have_http_status(:unprocessable_content)
    end

    it "preserves user locale preference on redirect" do
      user.update!(locale: "pl")

      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }

      expect(response.location).to include("locale=pl")
    end
  end

  describe "login with 2FA" do
    let(:user) { create(:user, password: "SecurePass1!", otp_module: :enabled) }

    before do
      # Enable OTP for the user
      user.otp_regenerate_secret
      user.save!
    end

    it "redirects to 2FA verification after password" do
      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }

      expect(response).to redirect_to(verify_two_factor_path)
    end

    it "does not sign in until 2FA verified" do
      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }
      follow_redirect!

      expect(controller.current_user).to be_nil
    end

    it "completes login with valid OTP code" do
      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }
      follow_redirect!

      valid_otp = user.otp_code

      post verify_two_factor_path, params: {
        user: { otp_code_token: valid_otp }
      }

      expect(response).to be_redirect
      follow_redirect!
      expect(controller.current_user).to eq(user)
    end

    it "rejects invalid OTP code" do
      post user_session_path, params: {
        user: { email: user.email, password: "SecurePass1!" }
      }
      follow_redirect!

      post verify_two_factor_path, params: {
        user: { otp_code_token: "000000" }
      }

      expect(response).to have_http_status(:unprocessable_content)
    end
  end

  describe "logout" do
    let(:user) { create(:user) }

    before { sign_in user }

    it "signs out the user" do
      delete destroy_user_session_path

      expect(response).to redirect_to(root_path)
      follow_redirect!
      expect(controller.current_user).to be_nil
    end
  end

  describe "protected routes" do
    it "redirects unauthenticated users to login" do
      get bots_path

      expect(response).to redirect_to(new_user_session_path)
    end

    it "allows authenticated users" do
      user = create(:user)
      sign_in user

      get bots_path

      expect(response).to have_http_status(:ok)
    end
  end
end
