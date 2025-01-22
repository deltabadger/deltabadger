class Sendgrid::UpdateFirstName < SendgridJob
  def perform(email, new_name)
    name = new_name.split.first.capitalize if new_name.present?
    contact = {
      email: email,
      first_name: name
    }.compact

    result = client.add_or_update_contacts(contacts: [contact])
    raise StandardError, result.errors if result.failure?
  end
end
