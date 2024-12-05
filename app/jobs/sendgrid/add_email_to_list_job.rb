class Sendgrid::AddEmailToListJob < Sendgrid::BaseJob
  def perform(email, list_name, name = nil)
    list_id = get_list_id(list_name)
    if list_id.nil?
      create_list_result = client.create_list(name: list_name)
      list_id = create_list_result.data.fetch('id')
    end

    name = name.split.first.capitalize if name.present?
    contact = {
      email: email,
      first_name: name
    }.compact
    result = client.add_or_update_contacts(list_ids: [list_id], contacts: [contact])
    raise StandardError, result.errors if result.failure?
  end
end
