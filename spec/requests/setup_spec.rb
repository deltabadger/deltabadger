require 'rails_helper'

RSpec.describe "Setup", type: :request do
  describe "first-time admin setup" do
    context "when no admin exists" do
      it "shows the setup form" do
        get new_setup_path
        expect(response).to have_http_status(:ok)
      end

      it "creates admin account with valid credentials" do
        expect {
          post setup_path, params: {
            user: { name: "Admin", email: "admin@example.com", password: "SecurePass1!" }
          }
        }.to change(User, :count).by(1)

        user = User.last
        expect(user.admin).to be true
        expect(user.confirmed_at).to be_present
        expect(response).to redirect_to(bots_path)
      end

      it "signs in the new admin after creation" do
        post setup_path, params: {
          user: { name: "Admin", email: "admin@example.com", password: "SecurePass1!" }
        }
        follow_redirect!

        expect(controller.current_user).to be_present
        expect(controller.current_user.admin).to be true
      end

      it "rejects invalid credentials" do
        post setup_path, params: {
          user: { name: "", email: "invalid", password: "weak" }
        }

        expect(response).to have_http_status(:unprocessable_content)
        expect(User.count).to eq(0)
      end

      it "rejects missing password" do
        post setup_path, params: {
          user: { name: "Admin", email: "admin@example.com", password: "" }
        }

        expect(response).to have_http_status(:unprocessable_content)
        expect(User.count).to eq(0)
      end
    end

    context "when admin already exists" do
      let!(:admin) { create(:user, admin: true) }

      it "redirects away from setup form" do
        get new_setup_path
        expect(response).to redirect_to(root_path)
      end

      it "prevents creating another admin" do
        expect {
          post setup_path, params: {
            user: { name: "Admin2", email: "admin2@example.com", password: "SecurePass1!" }
          }
        }.not_to change(User, :count)

        expect(response).to redirect_to(root_path)
      end
    end
  end
end
